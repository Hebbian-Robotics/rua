use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use hebbian_contracts::{ActionSequenceRequest, CompilationArtifact, CompilationDiagnostic};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::compiler::{CompilationTarget, CompiledProgram, RobotUseCompiler};
use crate::control_link::LinkHandle;

pub(crate) struct ProgramArtifact {
    pub id: String,
    pub source: ActionSequenceRequest,
    pub program: CompiledProgram,
    pub directory: PathBuf,
    instance_id: String,
    structural_compilation_seconds: f64,
    targets: BTreeMap<String, CompilationTarget>,
    _workspace: Arc<super::artifact_workspace::ArtifactWorkspace>,
}

#[derive(Default)]
pub(crate) struct ProgramStore {
    state: std::sync::Mutex<ProgramStoreState>,
}

#[derive(Default)]
struct ProgramStoreState {
    artifacts: VecDeque<Arc<ProgramArtifact>>,
    compiler: RobotUseCompiler,
    workspace: Option<Arc<super::artifact_workspace::ArtifactWorkspace>>,
}

pub(crate) enum ProgramCompilation {
    Ready(Arc<ProgramArtifact>),
    Rejected(CompilationDiagnostic),
}

pub(crate) enum ArtifactLookup {
    Found(Arc<ProgramArtifact>),
    Missing,
    Incompatible,
}

pub(crate) fn station_targets(
    links: &BTreeMap<String, LinkHandle>,
) -> anyhow::Result<BTreeMap<String, CompilationTarget>> {
    links
        .iter()
        .map(|(arm, link)| {
            let configuration = serde_json::to_vec(link.config())?;
            Ok((
                arm.clone(),
                CompilationTarget {
                    configuration_identity: format!("{:x}", Sha256::digest(configuration)),
                    joint_count: link.config().joint_count,
                    jaw_closing_axis: link.config().jaw_closing_axis,
                    supports_joints: !link.has_policy() && link.planner().is_some(),
                    supports_pose: link.has_policy() || link.planner().is_some(),
                    has_gripper: link.gripper().is_some(),
                },
            ))
        })
        .collect()
}

impl ProgramStore {
    pub fn compile(
        &self,
        instance_id: &str,
        targets: &BTreeMap<String, CompilationTarget>,
        source: ActionSequenceRequest,
    ) -> anyhow::Result<ProgramCompilation> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = std::time::Instant::now();
        let program = match state.compiler.compile(&source, targets) {
            Ok(program) => program,
            Err(diagnostic) => return Ok(ProgramCompilation::Rejected(diagnostic)),
        };
        let structural_compilation_seconds = started.elapsed().as_secs_f64();
        while state.artifacts.len() >= 128 {
            let removable = state
                .artifacts
                .iter()
                .position(|artifact| Arc::strong_count(artifact) == 1)
                .context("All retained compilation artifacts are in use")?;
            let artifact = &state.artifacts[removable];
            match std::fs::remove_dir_all(&artifact.directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("Cannot remove expired compilation artifact");
                }
            }
            state.artifacts.remove(removable);
        }
        if state.workspace.is_none() {
            state.workspace = Some(Arc::new(
                super::artifact_workspace::ArtifactWorkspace::open(instance_id)?,
            ));
        }
        let workspace = state
            .workspace
            .as_ref()
            .context("Compilation workspace was not created")?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = workspace.directory().join(&id);
        std::fs::create_dir_all(&directory)
            .context("Cannot create compilation artifact directory")?;
        let artifact = Arc::new(ProgramArtifact {
            id,
            source,
            program,
            directory,
            instance_id: instance_id.to_owned(),
            structural_compilation_seconds,
            targets: targets.clone(),
            _workspace: Arc::clone(workspace),
        });
        state.artifacts.push_back(Arc::clone(&artifact));
        drop(state);
        let contents = artifact.manifest_bytes()?;
        if let Err(error) = std::fs::write(artifact.directory.join("compilation.json"), contents) {
            let _ = std::fs::remove_dir(&artifact.directory);
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .artifacts
                .retain(|retained| retained.id != artifact.id);
            return Err(error).context("Cannot save compilation artifact");
        }
        Ok(ProgramCompilation::Ready(artifact))
    }

    pub fn lookup(
        &self,
        id: &str,
        targets: &BTreeMap<String, CompilationTarget>,
    ) -> ArtifactLookup {
        let artifact = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .artifacts
            .iter()
            .find(|artifact| artifact.id == id)
            .cloned();
        let Some(artifact) = artifact else {
            return ArtifactLookup::Missing;
        };
        if !artifact.directory.join("compilation.json").is_file() {
            return ArtifactLookup::Missing;
        }
        if artifact
            .program
            .arms
            .iter()
            .any(|arm| targets.get(arm) != artifact.targets.get(arm))
        {
            return ArtifactLookup::Incompatible;
        }
        ArtifactLookup::Found(artifact)
    }
}

impl ProgramArtifact {
    fn manifest_bytes(&self) -> anyhow::Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Manifest<'a> {
            schema_version: u32,
            compiler_version: &'static str,
            structural_compilation_seconds: f64,
            compilation_id: &'a str,
            station_instance_id: &'a str,
            targets: &'a BTreeMap<String, CompilationTarget>,
            source: &'a ActionSequenceRequest,
            compiled: &'a CompiledProgram,
            diagnostics: &'a [CompilationDiagnostic],
        }
        Ok(serde_json::to_vec(&Manifest {
            schema_version: 1,
            compiler_version: env!("CARGO_PKG_VERSION"),
            structural_compilation_seconds: self.structural_compilation_seconds,
            compilation_id: &self.id,
            station_instance_id: &self.instance_id,
            targets: &self.targets,
            source: &self.source,
            compiled: &self.program,
            diagnostics: &[],
        })?)
    }

    pub fn inspection(&self) -> CompilationArtifact {
        CompilationArtifact {
            schema_version: 1,
            compiler_version: env!("CARGO_PKG_VERSION").into(),
            structural_compilation_seconds: self.structural_compilation_seconds,
            compilation_id: self.id.clone(),
            station_instance_id: self.instance_id.clone(),
            targets: self.targets.clone(),
            source: self.source.clone(),
            compiled: self.program.clone(),
            diagnostics: vec![],
        }
    }

    pub fn persist_execution_updates(
        self: &Arc<Self>,
        mut updates: tokio::sync::watch::Receiver<hebbian_contracts::ActionSequenceRecord>,
    ) {
        let artifact = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let execution = updates.borrow_and_update().clone();
                let finished = !matches!(
                    execution.phase,
                    hebbian_contracts::ActionSequencePhase::Running { .. }
                );
                let artifact = Arc::clone(&artifact);
                match tokio::task::spawn_blocking(move || artifact.save_execution(&execution)).await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("Cannot persist execution outcome: {error:#}"),
                    Err(error) => eprintln!("Execution artifact worker failed: {error}"),
                }
                if finished || updates.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    pub fn record_prepared_step(
        self: &Arc<Self>,
        execution_id: &str,
        source_step: usize,
        source_operations: &BTreeMap<&str, &str>,
        actions: &[crate::action_step::PreparedArmAction],
    ) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct PreparedStepSnapshot<'a> {
            schema_version: u32,
            compilation_id: &'a str,
            execution_id: &'a str,
            source_step: usize,
            source_operations: &'a BTreeMap<&'a str, &'a str>,
            prepared_actions: &'a [crate::action_step::PreparedArmAction],
        }
        let contents = serde_json::to_vec(&PreparedStepSnapshot {
            schema_version: 1,
            compilation_id: &self.id,
            execution_id,
            source_step,
            source_operations,
            prepared_actions: actions,
        })?;
        self.persist_step_file(
            execution_id,
            &format!("step-{source_step}.prepared.json"),
            contents,
        );
        Ok(())
    }

    pub fn record_step_timing(
        self: &Arc<Self>,
        execution_id: &str,
        source_step: usize,
        preparation_seconds: f64,
        admission_seconds: f64,
    ) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct StepTiming<'a> {
            schema_version: u32,
            compilation_id: &'a str,
            execution_id: &'a str,
            source_step: usize,
            preparation_seconds: f64,
            admission_seconds: f64,
        }
        let contents = serde_json::to_vec(&StepTiming {
            schema_version: 1,
            compilation_id: &self.id,
            execution_id,
            source_step,
            preparation_seconds,
            admission_seconds,
        })?;
        self.persist_step_file(
            execution_id,
            &format!("step-{source_step}.timing.json"),
            contents,
        );
        Ok(())
    }

    fn persist_step_file(self: &Arc<Self>, execution_id: &str, filename: &str, contents: Vec<u8>) {
        let directory = self.directory.join("executions").join(execution_id);
        let destination = directory.join(filename);
        let artifact = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let temporary = destination.with_extension("json.pending");
            let saved = std::fs::create_dir_all(&directory)
                .and_then(|()| std::fs::write(&temporary, contents))
                .and_then(|()| std::fs::rename(&temporary, &destination));
            if let Err(error) = saved {
                eprintln!(
                    "Cannot persist step inspection for compilation {}: {error}",
                    artifact.id
                );
            }
        });
    }

    pub fn save_execution(
        &self,
        execution: &hebbian_contracts::ActionSequenceRecord,
    ) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct ExecutionManifest<'a> {
            schema_version: u32,
            compilation_id: &'a str,
            execution: &'a hebbian_contracts::ActionSequenceRecord,
        }
        let contents = serde_json::to_vec(&ExecutionManifest {
            schema_version: 1,
            compilation_id: &self.id,
            execution,
        })?;
        let directory = self.directory.join("executions").join(&execution.id);
        std::fs::create_dir_all(&directory)
            .context("Cannot create execution artifact directory")?;
        let destination = directory.join("record.json");
        let temporary = destination.with_extension("json.pending");
        std::fs::write(&temporary, contents).context("Cannot save execution record")?;
        std::fs::rename(&temporary, &destination).context("Cannot finalize execution record")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hebbian_contracts::{
        ActionSequenceArmAction, ActionSequencePhase, ActionSequenceRecord, ActionSequenceStep,
    };

    fn inputs() -> (ActionSequenceRequest, BTreeMap<String, CompilationTarget>) {
        (
            ActionSequenceRequest {
                steps: vec![ActionSequenceStep::single(
                    "left",
                    ActionSequenceArmAction {
                        movement: None,
                        gripper_opening: Some(0.8),
                        max_joint_speed: None,
                    },
                )],
                expected_revisions: BTreeMap::new(),
            },
            BTreeMap::from([(
                "left".into(),
                CompilationTarget {
                    configuration_identity: "configuration-a".into(),
                    joint_count: 1,
                    jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                    supports_joints: false,
                    supports_pose: false,
                    has_gripper: true,
                },
            )]),
        )
    }

    #[tokio::test]
    async fn saved_compilation_and_execution_keep_distinct_identities_and_source() {
        let instance = uuid::Uuid::new_v4().to_string();
        let store = ProgramStore::default();
        let (source, targets) = inputs();
        let ProgramCompilation::Ready(artifact) =
            store.compile(&instance, &targets, source).unwrap()
        else {
            panic!("valid source must compile");
        };
        let original_compilation =
            std::fs::read(artifact.directory.join("compilation.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&original_compilation).unwrap();
        assert_eq!(
            manifest,
            serde_json::to_value(artifact.inspection()).unwrap()
        );
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["compilation_id"], artifact.id);
        assert_eq!(
            manifest["source"]["steps"][0]["arms"]["left"]["gripper_opening"],
            0.8
        );
        assert_eq!(
            manifest["targets"]["left"]["configuration_identity"],
            "configuration-a"
        );
        let execution = ActionSequenceRecord {
            continuation: None,
            compilation_id: artifact.id.clone(),
            id: "execution-a".into(),
            arms: vec!["left".into()],
            requested_steps: artifact.source.steps.clone(),
            phase: ActionSequencePhase::Stopped {
                failure: hebbian_contracts::ExecutionFailure::Cancellation {
                    step: 0,
                    detail: "Canceled before admission".into(),
                },
            },
            steps: vec![],
        };
        artifact.save_execution(&execution).unwrap();
        artifact
            .record_prepared_step(
                &execution.id,
                0,
                &BTreeMap::from([(
                    "left",
                    artifact.program.steps[0]["left"]
                        .source_operation_id
                        .as_str(),
                )]),
                &[crate::action_step::PreparedArmAction {
                    arm: "left".into(),
                    movement: None,
                    gripper: Some(crate::action_step::PreparedGripper {
                        angle: 0.6,
                        joint_speed: None,
                    }),
                }],
            )
            .unwrap();
        let prepared_path = artifact
            .directory
            .join("executions/execution-a/step-0.prepared.json");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !prepared_path.is_file() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let prepared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(prepared_path).unwrap()).unwrap();
        assert_eq!(prepared["compilation_id"], artifact.id);
        assert_eq!(prepared["execution_id"], "execution-a");
        assert_eq!(prepared["source_step"], 0);
        assert_eq!(prepared["prepared_actions"][0]["arm"], "left");
        assert_eq!(prepared["prepared_actions"][0]["gripper"]["angle"], 0.6);

        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                artifact
                    .directory
                    .join("executions/execution-a/record.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["compilation_id"], artifact.id);
        assert_eq!(saved["execution"]["id"], "execution-a");
        assert_eq!(saved["execution"]["steps"], serde_json::json!([]));
        assert_eq!(
            std::fs::read(artifact.directory.join("compilation.json")).unwrap(),
            original_compilation
        );
        std::fs::remove_dir_all(std::env::temp_dir().join(format!("rua-compiler-{instance}")))
            .unwrap();
    }

    #[test]
    fn retention_preserves_in_use_artifacts_and_tolerates_external_cleanup() {
        let instance = uuid::Uuid::new_v4().to_string();
        let store = ProgramStore::default();
        let (source, targets) = inputs();
        let ProgramCompilation::Ready(active) =
            store.compile(&instance, &targets, source.clone()).unwrap()
        else {
            panic!("valid source must compile");
        };
        let ProgramCompilation::Ready(expiring) =
            store.compile(&instance, &targets, source.clone()).unwrap()
        else {
            panic!("valid source must compile");
        };
        let expired = expiring.directory.clone();
        let expired_id = expiring.id.clone();
        drop(expiring);
        std::fs::remove_dir_all(&expired).unwrap();
        for _ in 0..127 {
            store.compile(&instance, &targets, source.clone()).unwrap();
        }
        assert!(active.directory.join("compilation.json").is_file());
        assert!(!expired.exists());
        assert_eq!(store.state.lock().unwrap().artifacts.len(), 128);
        assert!(matches!(
            store.lookup(&active.id, &targets),
            ArtifactLookup::Found(_)
        ));
        assert!(matches!(
            store.lookup(&expired_id, &targets),
            ArtifactLookup::Missing
        ));
        let mut incompatible = targets.clone();
        incompatible.get_mut("left").unwrap().configuration_identity = "changed".into();
        assert!(matches!(
            store.lookup(&active.id, &incompatible),
            ArtifactLookup::Incompatible
        ));
        std::fs::remove_file(active.directory.join("compilation.json")).unwrap();
        assert!(matches!(
            store.lookup(&active.id, &targets),
            ArtifactLookup::Missing
        ));
        std::fs::remove_dir_all(std::env::temp_dir().join(format!("rua-compiler-{instance}")))
            .unwrap();
    }
}
