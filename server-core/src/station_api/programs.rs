use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use hebbian_contracts::{ActionSequenceRequest, CompilationArtifact, CompilationDiagnostic};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::compiler::{CompilationTarget, RobotUseCompiler};
use crate::control_link::LinkHandle;

const MAXIMUM_RETAINED_EXECUTION_COMPILATIONS: usize = 128;
const MAXIMUM_RETAINED_INSPECTION_COMPILATIONS: usize = 128;

pub(crate) struct ProgramArtifact {
    pub compilation: CompilationArtifact,
    files: Option<ArtifactFiles>,
}

struct ArtifactFiles {
    directory: PathBuf,
    _workspace: Arc<super::artifact_workspace::ArtifactWorkspace>,
}

#[derive(Clone, Copy)]
pub(crate) enum CompilationRetention {
    InMemory,
    WithInspectionFiles,
}

#[derive(Default)]
pub(crate) struct ProgramStore {
    state: Mutex<ProgramStoreState>,
    inspection_workspace: Mutex<Option<Arc<super::artifact_workspace::ArtifactWorkspace>>>,
}

#[derive(Default)]
struct ProgramStoreState {
    execution_compilations: VecDeque<Arc<ProgramArtifact>>,
    inspection_compilations: VecDeque<Arc<ProgramArtifact>>,
    compiler: RobotUseCompiler,
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
        retention: CompilationRetention,
    ) -> anyhow::Result<ProgramCompilation> {
        let (program, structural_compilation_seconds) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let started = std::time::Instant::now();
            let program = match state.compiler.compile(&source, targets) {
                Ok(program) => program,
                Err(diagnostic) => return Ok(ProgramCompilation::Rejected(diagnostic)),
            };
            (program, started.elapsed().as_secs_f64())
        };
        let id = uuid::Uuid::new_v4().to_string();
        // INVARIANT: Serialize explicit writes and cleanup under a lock used only by inspection.
        let mut workspace = match retention {
            CompilationRetention::InMemory => None,
            CompilationRetention::WithInspectionFiles => Some(
                self.inspection_workspace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
        };
        let files = if let Some(workspace) = workspace.as_deref_mut() {
            if workspace.is_none() {
                *workspace = Some(Arc::new(
                    super::artifact_workspace::ArtifactWorkspace::open(instance_id)?,
                ));
            }
            workspace.as_ref().map(|workspace| ArtifactFiles {
                directory: workspace.directory().join(&id),
                _workspace: Arc::clone(workspace),
            })
        } else {
            None
        };
        let artifact = Arc::new(ProgramArtifact {
            compilation: CompilationArtifact {
                schema_version: 1,
                compiler_version: env!("CARGO_PKG_VERSION").into(),
                structural_compilation_seconds,
                compilation_id: id,
                station_instance_id: instance_id.to_owned(),
                targets: targets.clone(),
                source,
                compiled: program,
                diagnostics: Vec::new(),
            },
            files,
        });
        if let Some(directory) = artifact.directory() {
            let contents = serde_json::to_vec(&artifact.compilation)?;
            std::fs::create_dir(directory)
                .context("Cannot create compilation artifact directory in the locked workspace")?;
            // INVARIANT: A failed write cannot evict a retained compilation.
            if let Err(error) = std::fs::write(directory.join("compilation.json"), contents) {
                let _ = std::fs::remove_dir_all(directory);
                return Err(error).context("Cannot save compilation artifact");
            }
        }
        let expired = match self.retain(Arc::clone(&artifact)) {
            Ok(expired) => expired,
            Err(error) => {
                if let Some(directory) = artifact.directory() {
                    let _ = std::fs::remove_dir_all(directory);
                }
                return Err(error);
            }
        };
        if let Some(directory) = expired.as_ref().and_then(|artifact| artifact.directory())
            && let Err(error) = std::fs::remove_dir_all(directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "Cannot remove expired compilation artifact {}: {error}",
                directory.display()
            );
        }
        Ok(ProgramCompilation::Ready(artifact))
    }

    fn retain(
        &self,
        artifact: Arc<ProgramArtifact>,
    ) -> anyhow::Result<Option<Arc<ProgramArtifact>>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (artifacts, capacity) = if artifact.files.is_some() {
            (
                &mut state.inspection_compilations,
                MAXIMUM_RETAINED_INSPECTION_COMPILATIONS,
            )
        } else {
            (
                &mut state.execution_compilations,
                MAXIMUM_RETAINED_EXECUTION_COMPILATIONS,
            )
        };
        let expired = if artifacts.len() >= capacity {
            let index = artifacts
                .iter()
                .position(|artifact| Arc::strong_count(artifact) == 1)
                .context("All retained compilation artifacts are in use")?;
            artifacts.remove(index)
        } else {
            None
        };
        artifacts.push_back(artifact);
        Ok(expired)
    }

    pub fn lookup(
        &self,
        id: &str,
        targets: &BTreeMap<String, CompilationTarget>,
    ) -> ArtifactLookup {
        let artifact = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state
                .inspection_compilations
                .iter()
                .chain(&state.execution_compilations)
                .find(|artifact| artifact.compilation.compilation_id == id)
                .cloned()
        };
        let Some(artifact) = artifact else {
            return ArtifactLookup::Missing;
        };
        if artifact
            .compilation
            .compiled
            .arms
            .iter()
            .any(|arm| targets.get(arm) != artifact.compilation.targets.get(arm))
        {
            return ArtifactLookup::Incompatible;
        }
        ArtifactLookup::Found(artifact)
    }
}

impl ProgramArtifact {
    pub(super) fn directory(&self) -> Option<&std::path::Path> {
        self.files.as_ref().map(|files| files.directory.as_path())
    }

    pub fn persist_execution_updates(
        self: &Arc<Self>,
        mut updates: tokio::sync::watch::Receiver<hebbian_contracts::ActionSequenceRecord>,
    ) {
        if self.files.is_none() {
            return;
        }
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
            compilation_id: &self.compilation.compilation_id,
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
            compilation_id: &self.compilation.compilation_id,
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
        let Some(directory) = self.directory() else {
            return;
        };
        let directory = directory.join("executions").join(execution_id);
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
                    artifact.compilation.compilation_id
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
        let Some(directory) = self.directory() else {
            return Ok(());
        };
        let contents = serde_json::to_vec(&ExecutionManifest {
            schema_version: 1,
            compilation_id: &self.compilation.compilation_id,
            execution,
        })?;
        let directory = directory.join("executions").join(&execution.id);
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
        let ProgramCompilation::Ready(artifact) = store
            .compile(
                &instance,
                &targets,
                source,
                CompilationRetention::WithInspectionFiles,
            )
            .unwrap()
        else {
            panic!("valid source must compile");
        };
        let directory = artifact.directory().unwrap();
        let original_compilation = std::fs::read(directory.join("compilation.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&original_compilation).unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(
            manifest["compilation_id"],
            artifact.compilation.compilation_id
        );
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
            compilation_id: artifact.compilation.compilation_id.clone(),
            id: "execution-a".into(),
            arms: vec!["left".into()],
            requested_steps: artifact.compilation.source.steps.clone(),
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
                    artifact.compilation.compiled.steps[0]["left"]
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
        let prepared_path = directory.join("executions/execution-a/step-0.prepared.json");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !prepared_path.is_file() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let prepared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(prepared_path).unwrap()).unwrap();
        assert_eq!(
            prepared["compilation_id"],
            artifact.compilation.compilation_id
        );
        assert_eq!(prepared["execution_id"], "execution-a");
        assert_eq!(prepared["source_step"], 0);
        assert_eq!(prepared["prepared_actions"][0]["arm"], "left");
        assert_eq!(prepared["prepared_actions"][0]["gripper"]["angle"], 0.6);

        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("executions/execution-a/record.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["compilation_id"], artifact.compilation.compilation_id);
        assert_eq!(saved["execution"]["id"], "execution-a");
        assert_eq!(saved["execution"]["steps"], serde_json::json!([]));
        assert_eq!(
            std::fs::read(directory.join("compilation.json")).unwrap(),
            original_compilation
        );
        std::fs::remove_dir_all(std::env::temp_dir().join(format!("rua-compiler-{instance}")))
            .unwrap();
    }

    #[test]
    fn inspection_retention_survives_in_memory_compilation_and_external_cleanup() {
        let instance = uuid::Uuid::new_v4().to_string();
        let store = ProgramStore::default();
        let (source, targets) = inputs();
        let compile_with_files = || {
            store.compile(
                &instance,
                &targets,
                source.clone(),
                CompilationRetention::WithInspectionFiles,
            )
        };
        let ProgramCompilation::Ready(active) = compile_with_files().unwrap() else {
            panic!("valid source must compile");
        };
        let ProgramCompilation::Ready(expiring) = compile_with_files().unwrap() else {
            panic!("valid source must compile");
        };
        let expired = expiring.directory().unwrap().to_owned();
        let expired_id = expiring.compilation.compilation_id.clone();
        drop(expiring);
        std::fs::remove_dir_all(&expired).unwrap();
        let ProgramCompilation::Ready(oldest_retained) = compile_with_files().unwrap() else {
            panic!("valid source must compile");
        };
        let oldest_retained_id = oldest_retained.compilation.compilation_id.clone();
        drop(oldest_retained);
        for _ in 0..126 {
            compile_with_files().unwrap();
        }
        assert!(
            active
                .directory()
                .unwrap()
                .join("compilation.json")
                .is_file()
        );
        assert!(!expired.exists());
        assert!(matches!(
            store.lookup(&active.compilation.compilation_id, &targets),
            ArtifactLookup::Found(_)
        ));
        assert!(matches!(
            store.lookup(&expired_id, &targets),
            ArtifactLookup::Missing
        ));

        let workspace = active.directory().unwrap().parent().unwrap();
        let displaced_workspace = workspace.with_extension("moved");
        std::fs::rename(workspace, &displaced_workspace).unwrap();
        assert!(compile_with_files().is_err());
        assert!(matches!(
            store.lookup(&oldest_retained_id, &targets),
            ArtifactLookup::Found(_)
        ));
        let implicit = store.compile(&instance, &targets, source, CompilationRetention::InMemory);
        if workspace.exists() {
            std::fs::remove_dir_all(workspace).unwrap();
        }
        std::fs::rename(displaced_workspace, workspace).unwrap();
        let ProgramCompilation::Ready(implicit) = implicit.unwrap() else {
            panic!("valid source must compile even when inspection files are unavailable");
        };
        assert!(matches!(
            store.lookup(&implicit.compilation.compilation_id, &targets),
            ArtifactLookup::Found(_)
        ));
        assert!(matches!(
            store.lookup(&oldest_retained_id, &targets),
            ArtifactLookup::Found(_)
        ));

        let mut incompatible = targets.clone();
        incompatible.get_mut("left").unwrap().configuration_identity = "changed".into();
        assert!(matches!(
            store.lookup(&active.compilation.compilation_id, &incompatible),
            ArtifactLookup::Incompatible
        ));
        std::fs::remove_file(active.directory().unwrap().join("compilation.json")).unwrap();
        assert!(matches!(
            store.lookup(&active.compilation.compilation_id, &targets),
            ArtifactLookup::Found(_)
        ));
        std::fs::remove_dir_all(std::env::temp_dir().join(format!("rua-compiler-{instance}")))
            .unwrap();
    }
}
