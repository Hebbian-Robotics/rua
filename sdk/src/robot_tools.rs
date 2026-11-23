//! Framework-independent robot tool definitions and execution.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hebbian_contracts::{ObservationAnswer, ObservationRequest};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use hebbian_contracts::{
    ActionSequencePhase, ActionSequenceRequest, ArmRecord, CommandOutcome, ModelOutcome,
    ObservationSource, ObserveArguments, PreviewPoseArguments, RobotToolDefinition,
    RobotToolOperation, SequenceToolConfiguration, StationCommand, StopArguments,
    action_sequence_tool_schema, parse_sequence_tool_arguments, robot_tool_catalog,
    robot_tool_context,
};
use serde_json::Value;

use crate::sequence_images::{SequenceCapture, SequenceImages};
use crate::tool_operations::{ToolOperation, ToolOperationAnswer};
use crate::{CommandOptions, Error, LeaseGuard};

/// A model-produced call. Arguments are parsed before any station operation.
#[derive(Debug, Clone)]
pub struct RobotToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// An expected outcome of a call, retaining full payloads in original station units.
#[derive(Debug)]
pub enum RobotToolOutcome {
    Returned(Value),
    InvalidInput(String),
    /// The operation failed without a confirmed outcome.
    Failed(Error),
    /// An earlier operation was interrupted or did not complete.
    NotExecuted,
    Refused {
        reason: hebbian_contracts::RejectionReason,
        detail: String,
    },
    Incomplete(Value),
    /// A hard fault ended the operation and powered off the motors.
    Faulted(Value),
    /// A communication interruption ended the operation.
    Interrupted(Value),
}

/// A call result retains its identifier so a framework can associate it with the request.
#[derive(Debug)]
pub struct RobotToolResult {
    pub call_id: String,
    pub outcome: RobotToolOutcome,
    /// Explicitly requested observation images, labelled for application-owned model messages.
    pub cameras: BTreeMap<String, Arc<[u8]>>,
}

impl RobotToolResult {
    fn new(call_id: String, outcome: RobotToolOutcome) -> Self {
        Self {
            call_id,
            outcome,
            cameras: BTreeMap::new(),
        }
    }

    /// Complete station payload in original units, or the execution error/refusal.
    /// Pass this value to a custom formatter without changing the outcome variant.
    #[must_use]
    pub fn data(&self) -> Value {
        match &self.outcome {
            RobotToolOutcome::Returned(value)
            | RobotToolOutcome::Incomplete(value)
            | RobotToolOutcome::Faulted(value)
            | RobotToolOutcome::Interrupted(value) => value.clone(),
            RobotToolOutcome::InvalidInput(detail) => Value::String(detail.clone()),
            RobotToolOutcome::Failed(error) => Value::String(error.to_string()),
            RobotToolOutcome::NotExecuted => Value::String("Tool call was not executed".into()),
            RobotToolOutcome::Refused { reason, detail } => {
                serde_json::json!({ "status": "refused", "reason": reason, "detail": detail })
            }
        }
    }

    /// Default concise model view. Application data and outcome variants remain intact.
    /// For a custom view, compose ordinary functions over [`Self::data`].
    #[must_use]
    pub fn model_data(&self) -> Value {
        crate::response_format::concise_response(self.data())
    }
}

/// Current station data and the conventions needed to interpret tool calls.
#[derive(Debug, serde::Serialize)]
pub struct RobotContext {
    pub conventions: Value,
    pub station: Value,
}

/// A tool controller bound to an existing lease and default arm.
#[derive(Clone)]
pub struct RobotTools<'a> {
    lease: &'a LeaseGuard,
    arm: String,
    descriptions: BTreeMap<String, String>,
    sequence: SequenceToolConfiguration,
    operations: Vec<RobotToolOperation>,
    cameras: Vec<String>,
    sequence_images: SequenceImages,
}

impl<'a> RobotTools<'a> {
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "The built-in default mapping selects unique catalog operations and a valid default sequence."
    )]
    pub fn new(lease: &'a LeaseGuard, arm: impl Into<String>) -> Self {
        let tools = Self {
            lease,
            arm: arm.into(),
            descriptions: BTreeMap::new(),
            cameras: Vec::new(),
            sequence_images: SequenceImages::default(),
            sequence: SequenceToolConfiguration::default(),
            operations: hebbian_contracts::all_robot_tool_capabilities()
                .iter()
                .map(hebbian_contracts::RobotToolCapability::operation)
                .collect(),
        };
        tools
            .map_tools(hebbian_contracts::default_robot_tool_mapping)
            .expect("default tool mapping")
    }

    /// Select cameras sampled during model action sequences, independently of recording.
    #[must_use]
    pub fn with_cameras(mut self, cameras: Vec<String>) -> Self {
        self.cameras = cameras
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    /// Selected historical images for the application's next model observation.
    #[must_use]
    pub fn sequence_images(&self) -> Option<crate::SequenceImageHistory> {
        self.sequence_images.snapshot()
    }

    /// # Errors
    /// The sequence configuration enables no actions or has invalid arm names.
    pub fn with_sequence(
        mut self,
        sequence: SequenceToolConfiguration,
    ) -> std::result::Result<Self, String> {
        sequence.validate()?;
        self.sequence = sequence;
        Ok(self)
    }

    /// Select independently available tools, including standalone operations.
    #[must_use]
    pub fn with_operations(mut self, operations: BTreeSet<RobotToolOperation>) -> Self {
        self.operations = hebbian_contracts::all_robot_tool_capabilities()
            .into_iter()
            .map(|capability| capability.operation())
            .filter(|operation| operations.contains(operation))
            .collect();
        self
    }

    /// Transform capabilities while sharing the same station and lease.
    ///
    /// # Errors
    /// The transformation repeats a tool or leaves an invalid sequence definition.
    #[expect(
        clippy::expect_used,
        reason = "Private operations are selected from the shared catalog and remain unique after validated transformations."
    )]
    pub fn map_tools(
        mut self,
        transform: impl FnOnce(
            Vec<hebbian_contracts::RobotToolCapability>,
        ) -> Vec<hebbian_contracts::RobotToolCapability>,
    ) -> std::result::Result<Self, String> {
        use hebbian_contracts::RobotToolCapability;
        let mut capabilities_by_operation: BTreeMap<_, _> =
            hebbian_contracts::all_robot_tool_capabilities()
                .into_iter()
                .map(|capability| (capability.operation(), capability))
                .collect();
        let capabilities = self
            .operations
            .iter()
            .map(|operation| {
                match capabilities_by_operation
                    .remove(operation)
                    .expect("operations come from the shared tool catalog")
                {
                    RobotToolCapability::ActionSequence(_) => {
                        RobotToolCapability::ActionSequence(self.sequence.clone())
                    }
                    capability => capability,
                }
            })
            .collect();
        let transformed = transform(capabilities);
        let mut operations = Vec::with_capacity(transformed.len());
        let mut seen_operations = BTreeSet::new();
        for capability in transformed {
            let operation = capability.operation();
            if !seen_operations.insert(operation) {
                return Err("A tool appears more than once".into());
            }
            if let RobotToolCapability::ActionSequence(sequence) = capability {
                sequence.validate()?;
                self.sequence = sequence;
            }
            operations.push(operation);
        }
        self.operations = operations;
        Ok(self)
    }

    /// Override descriptions for this controller without changing command behavior.
    #[must_use]
    pub fn with_descriptions(mut self, descriptions: BTreeMap<String, String>) -> Self {
        self.descriptions = descriptions;
        self
    }

    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Private operations come from the catalog; sequence configuration is validated before storage."
    )]
    pub fn definitions(&self) -> Vec<RobotToolDefinition> {
        let mut definitions_by_operation: BTreeMap<_, _> = robot_tool_catalog()
            .into_iter()
            .map(|definition| (definition.operation, definition))
            .collect();
        self.operations
            .iter()
            .map(|operation| {
                definitions_by_operation
                    .remove(operation)
                    .expect("operations come from the shared tool catalog")
            })
            .map(|mut definition| {
                if definition.name == "action_sequence" {
                    definition.parameters = action_sequence_tool_schema(&self.sequence)
                        .expect("validated sequence configuration");
                }
                if let Some(description) = self.descriptions.get(definition.name) {
                    definition.description.clone_from(description);
                }
                definition
            })
            .collect()
    }

    /// Read the current station context and shared robot conventions.
    ///
    /// # Errors
    /// Returns a station connection error when state is unavailable.
    pub async fn context(&self) -> crate::Result<RobotContext> {
        Ok(RobotContext {
            conventions: robot_tool_context(),
            station: crate::response_format::concise_response(
                serde_json::to_value(self.lease.station().state().await?)
                    .map_err(|error| Error::Connection(error.to_string()))?,
            ),
        })
    }

    /// Execute one call and wait for its individual operation to finish.
    ///
    /// # Errors
    /// Returns a connection, HTTP, or timeout error when the station cannot be reached.
    pub async fn execute(
        &self,
        call: RobotToolCall,
        options: CommandOptions,
    ) -> crate::Result<RobotToolResult> {
        let mut arguments = call.arguments.clone();
        let operation =
            match convert_model_arguments_to_station_units(&mut arguments).and_then(|()| {
                normalize_arguments(&mut arguments);
                self.parse_operation(&call.name, arguments, &options)
            }) {
                Ok(operation) => operation,
                Err(detail) => {
                    return Ok(RobotToolResult {
                        cameras: std::collections::BTreeMap::new(),
                        call_id: call.id,
                        outcome: RobotToolOutcome::InvalidInput(detail),
                    });
                }
            };
        let station = self.lease.station();
        let answer = match operation {
            ToolOperation::PosePreviews { poses } => ToolOperationAnswer::PosePreviews {
                results: station
                    .preview_pose_candidates(
                        &self.arm,
                        poses.into_iter().map(|pose| (None, pose)).collect(),
                    )
                    .await?,
            },
            ToolOperation::MoveArms { command, .. } => {
                match self.lease.run_arms_records(command, options.clone()).await {
                    Ok(records) => {
                        return Ok(RobotToolResult {
                            cameras: std::collections::BTreeMap::new(),
                            call_id: call.id,
                            outcome: command_records_outcome(records, true)?,
                        });
                    }
                    Err(Error::Refused { reason, detail }) => {
                        ToolOperationAnswer::Refused { reason, detail }
                    }
                    Err(error) => return Err(error),
                }
            }
            ToolOperation::SetGripper {
                arm,
                opening,
                max_joint_speed,
            } => {
                let outcome = self
                    .run_single_command(
                        &arm,
                        StationCommand::SetGripper {
                            opening,
                            max_joint_speed,
                        },
                        options,
                    )
                    .await?;
                return Ok(RobotToolResult::new(call.id, outcome));
            }
            ToolOperation::ActionSequence { action_sequence } => {
                return Ok(RobotToolResult {
                    cameras: std::collections::BTreeMap::new(),
                    call_id: call.id.clone(),
                    outcome: self.run_sequence(&call.id, action_sequence).await?,
                });
            }
            ToolOperation::Stop { arm } => {
                let outcome = single_command_outcome(
                    &arm,
                    station.run_with(&arm, StationCommand::Stop, options).await,
                )?;
                return Ok(RobotToolResult::new(call.id, outcome));
            }
            ToolOperation::Rehome { arm } => {
                let outcome = self.rehome(&arm, options).await?;
                return Ok(RobotToolResult::new(call.id, outcome));
            }
            ToolOperation::Observe {
                source,
                wait_seconds,
            } => {
                return self.observe_tool(call.id, source, wait_seconds).await;
            }
        };
        let outcome = self.operation_outcome(answer).await?;
        Ok(RobotToolResult {
            cameras: std::collections::BTreeMap::new(),
            call_id: call.id,
            outcome,
        })
    }

    async fn observe_tool(
        &self,
        call_id: String,
        source: ObservationSource,
        wait_seconds: f64,
    ) -> crate::Result<RobotToolResult> {
        tokio::time::sleep(std::time::Duration::from_secs_f64(wait_seconds)).await;
        if source == ObservationSource::LastSequence {
            return Ok(self.history_observation(call_id));
        }
        let station = self.lease.station();
        let (state, frames) = match station
            .capture_observation_http(&ObservationRequest {
                arms: Vec::new(),
                required_cameras: self
                    .cameras
                    .iter()
                    .map(|name| hebbian_contracts::SnapshotName::try_from(name.clone()))
                    .collect::<Result<_, _>>()?,
                optional_cameras: Vec::new(),
            })
            .await?
        {
            ObservationAnswer::ObservationCaptured { state, cameras, .. } => (state, cameras),
            ObservationAnswer::ObservationArmDisconnected { arm } => {
                return Err(crate::Error::Connection(format!(
                    "Arm {arm} is disconnected"
                )));
            }
            ObservationAnswer::ObservationRevisionChanged { arm } => {
                return Err(crate::Error::Refused {
                    reason: hebbian_contracts::RejectionReason::StaleRevision,
                    detail: format!("Arm {arm} changed during observation"),
                });
            }
            ObservationAnswer::ObservationRequiredCameraUnavailable { camera } => {
                return Err(crate::Error::CameraUnavailable {
                    name: camera,
                    listed: Vec::new(),
                    last_error: None,
                });
            }
        };
        let cameras = frames
            .into_iter()
            .map(|(name, frame)| {
                STANDARD
                    .decode(frame.jpeg_base64)
                    .map(|jpeg| (name, jpeg.into()))
                    .map_err(|error| {
                        crate::Error::Connection(format!("Invalid observation frame: {error}"))
                    })
            })
            .collect::<crate::Result<BTreeMap<_, _>>>()?;

        Ok(RobotToolResult {
            call_id,
            cameras,
            outcome: self
                .operation_outcome(ToolOperationAnswer::Observation { state })
                .await?,
        })
    }

    fn history_observation(&self, call_id: String) -> RobotToolResult {
        let Some(history) = self.sequence_images() else {
            return RobotToolResult {
                call_id,
                cameras: std::collections::BTreeMap::new(),
                outcome: RobotToolOutcome::InvalidInput(
                    "No sequence image history is available; none was captured or it expired."
                        .into(),
                ),
            };
        };
        let cameras = history
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                (
                    format!(
                        "Sequence {} image {}: {}, +{:.3}s, {}; retrieved {:?} Unix seconds",
                        history.call_id,
                        index + 1,
                        image.camera,
                        image.elapsed_seconds,
                        image.boundary,
                        image.retrieved_between_unix_seconds
                    ),
                    image.jpeg.clone(),
                )
            })
            .collect();
        let data = serde_json::json!({
            "source": "last_sequence", "sequence_call_id": history.call_id,
            "missed_samples": history.missed_samples, "truncated": history.truncated,
            "images": history.images.iter().map(|image| serde_json::json!({
                "camera": image.camera, "elapsed_seconds": image.elapsed_seconds,
                "retrieved_between_unix_seconds": image.retrieved_between_unix_seconds,
                "boundary": image.boundary,
            })).collect::<Vec<_>>(),
        });
        RobotToolResult {
            call_id,
            cameras,
            outcome: RobotToolOutcome::Returned(data),
        }
    }

    async fn run_sequence(
        &self,
        call_id: &str,
        request: ActionSequenceRequest,
    ) -> crate::Result<RobotToolOutcome> {
        let capture = SequenceCapture::start(
            self.sequence_images.clone(),
            self.lease.station().clone(),
            self.cameras.clone(),
            call_id.to_owned(),
        );
        let outcome = async {
            let answer = match self.lease.start_action_sequence(request).await {
                Ok(action_sequence_id) => {
                    if let Some(capture) = &capture {
                        capture.set_sequence_id(action_sequence_id.clone());
                    }
                    ToolOperationAnswer::ActionSequence { action_sequence_id }
                }
                Err(Error::Refused { reason, detail }) => {
                    ToolOperationAnswer::Refused { reason, detail }
                }
                Err(error) => return Err(error),
            };
            self.operation_outcome(answer).await
        }
        .await;
        if let Some(capture) = capture {
            capture.finish().await;
        }
        outcome
    }

    async fn rehome(&self, arm: &str, options: CommandOptions) -> crate::Result<RobotToolOutcome> {
        single_command_outcome(arm, self.lease.rehome_with(arm, options).await)
    }

    async fn run_single_command(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
    ) -> crate::Result<RobotToolOutcome> {
        single_command_outcome(arm, self.lease.run_with_lease(arm, command, options).await)
    }

    /// Execute model calls concurrently and return results in input order.
    /// Individual errors are returned as [`RobotToolOutcome::Failed`].
    pub async fn execute_calls(
        &self,
        calls: Vec<RobotToolCall>,
        options: CommandOptions,
    ) -> Vec<RobotToolResult> {
        futures_util::future::join_all(calls.into_iter().map(|call| {
            let call_id = call.id.clone();
            let options = options.clone();
            async move {
                self.execute(call, options)
                    .await
                    .unwrap_or_else(|error| RobotToolResult {
                        cameras: std::collections::BTreeMap::new(),
                        call_id,
                        outcome: RobotToolOutcome::Failed(error),
                    })
            }
        }))
        .await
    }

    fn parse_operation(
        &self,
        name: &str,
        arguments: Value,
        options: &CommandOptions,
    ) -> std::result::Result<ToolOperation, String> {
        fn parse<T: serde::de::DeserializeOwned>(value: Value) -> std::result::Result<T, String> {
            serde_json::from_value(value).map_err(|error| error.to_string())
        }
        if !self
            .definitions()
            .iter()
            .any(|definition| definition.name == name)
        {
            return Err(format!("Tool {name} is not available"));
        }
        Ok(match name {
            "preview_pose" => ToolOperation::PosePreviews {
                poses: parse::<PreviewPoseArguments>(arguments)?
                    .poses
                    .into_iter()
                    .map(|candidate| candidate.pose)
                    .collect(),
            },
            "move_arms" => ToolOperation::MoveArms {
                command: parse(arguments)?,
            },
            "set_arm_gripper" => {
                let mut fields = arguments.as_object().cloned().ok_or("expected an object")?;
                let Some(Value::String(arm)) = fields.remove("arm") else {
                    return Err("arm must be a string".into());
                };
                if fields.contains_key("kind") {
                    return Err("kind is supplied by the tool name".into());
                }
                fields.insert("kind".into(), Value::String("set_gripper".into()));
                let StationCommand::SetGripper {
                    opening,
                    max_joint_speed,
                } = parse(Value::Object(fields))?
                else {
                    return Err("expected a gripper command".into());
                };
                ToolOperation::SetGripper {
                    arm,
                    opening,
                    max_joint_speed,
                }
            }
            "action_sequence" => {
                let steps = parse_sequence_tool_arguments(arguments, &self.sequence, &self.arm)?;
                ToolOperation::ActionSequence {
                    action_sequence: ActionSequenceRequest {
                        steps,
                        expected_revisions: options.expected_revisions.clone(),
                    },
                }
            }
            "stop" => {
                let _: StopArguments = parse(arguments)?;
                ToolOperation::Stop {
                    arm: self.arm.clone(),
                }
            }
            "rehome" => {
                let _: StopArguments = parse(arguments)?;
                ToolOperation::Rehome {
                    arm: self.arm.clone(),
                }
            }
            "observe" => {
                let input: ObserveArguments = parse(arguments)?;
                if !(0.0..=10.0).contains(&input.wait_seconds) {
                    return Err("wait_seconds must be between 0 and 10".into());
                }
                ToolOperation::Observe {
                    source: input.source,
                    wait_seconds: input.wait_seconds,
                }
            }
            _ => return Err(format!("unknown tool {name}")),
        })
    }

    async fn operation_outcome(
        &self,
        answer: ToolOperationAnswer,
    ) -> crate::Result<RobotToolOutcome> {
        let station = self.lease.station();
        let value = match answer {
            ToolOperationAnswer::Refused { reason, detail } => {
                return Ok(RobotToolOutcome::Refused { reason, detail });
            }
            ToolOperationAnswer::PosePreviews { results } => {
                serde_json::json!({ "results": results.into_iter().map(|candidate| serde_json::json!({"answer": candidate.answer})).collect::<Vec<_>>() })
            }
            ToolOperationAnswer::Observation { state } => {
                serde_json::to_value(state).map_err(|error| {
                    Error::Connection(format!("cannot serialize tool result: {error}"))
                })?
            }
            ToolOperationAnswer::ActionSequence { action_sequence_id } => {
                let sequence = station
                    .wait_for_action_sequence(&action_sequence_id)
                    .await?;
                let incomplete = !matches!(sequence.phase, ActionSequencePhase::Completed);
                let value = serde_json::to_value(&sequence).map_err(|error| {
                    Error::Connection(format!("cannot serialize tool result: {error}"))
                })?;
                return Ok(classify_motion_outcome(
                    sequence
                        .steps
                        .iter()
                        .flatten()
                        .filter_map(|record| record.record.outcome()),
                    incomplete,
                    value,
                ));
            }
        };
        Ok(RobotToolOutcome::Returned(value))
    }
}

fn single_command_outcome(
    arm: &str,
    result: crate::Result<hebbian_contracts::CommandRecord>,
) -> crate::Result<RobotToolOutcome> {
    let record = match result {
        Ok(record) => record,
        Err(Error::Incomplete { record, .. }) => *record,
        Err(Error::Refused { reason, detail }) => {
            return Ok(RobotToolOutcome::Refused { reason, detail });
        }
        Err(error) => return Err(error),
    };
    command_records_outcome(
        vec![ArmRecord {
            arm: arm.to_owned(),
            record,
        }],
        false,
    )
}

fn command_records_outcome(
    records: Vec<ArmRecord>,
    grouped: bool,
) -> crate::Result<RobotToolOutcome> {
    let mut payloads = Vec::with_capacity(records.len());
    let mut limited = false;
    let mut complete = true;
    let mut stopped_short = false;
    let group_id = records
        .first()
        .and_then(|record| record.record.group_id())
        .map(str::to_owned);
    for record in &records {
        limited |= record.record.was_limited();
        let outcome = record.record.outcome().map(ModelOutcome::from);
        complete &= outcome == Some(ModelOutcome::Completed);
        stopped_short |= outcome == Some(ModelOutcome::StoppedShort);
        payloads.push(serde_json::to_value(&record.record).map_err(|error| {
            Error::Connection(format!("cannot serialize tool result: {error}"))
        })?);
    }
    let incomplete = limited || !complete;
    let value = if grouped {
        let status = if complete {
            if limited { "limited" } else { "reached" }
        } else if stopped_short {
            "stopped_short"
        } else {
            "partial"
        };
        serde_json::json!({ "group_id": group_id, "status": status, "commands": payloads })
    } else {
        payloads
            .into_iter()
            .next()
            .ok_or_else(|| Error::Connection("missing command record".into()))?
    };
    Ok(classify_motion_outcome(
        records.iter().filter_map(|record| record.record.outcome()),
        incomplete,
        value,
    ))
}

fn classify_motion_outcome(
    outcomes: impl Iterator<Item = CommandOutcome>,
    incomplete: bool,
    value: Value,
) -> RobotToolOutcome {
    let mut interrupted = false;
    for outcome in outcomes.map(ModelOutcome::from) {
        if outcome == ModelOutcome::Faulted {
            return RobotToolOutcome::Faulted(value);
        }
        interrupted |= outcome == ModelOutcome::Interrupted;
    }
    if interrupted {
        RobotToolOutcome::Interrupted(value)
    } else if incomplete {
        RobotToolOutcome::Incomplete(value)
    } else {
        RobotToolOutcome::Returned(value)
    }
}

// Model-facing units are converted before station variants are parsed.
fn convert_model_arguments_to_station_units(value: &mut Value) -> std::result::Result<(), String> {
    use hebbian_contracts::{MODEL_INPUT_UNITS, ModelInputUnit};

    match value {
        Value::Object(object) => {
            for ModelInputUnit { station_field, .. } in MODEL_INPUT_UNITS {
                if object.contains_key(station_field) {
                    return Err(format!("use explicit model units for {station_field}"));
                }
            }
            for (key, child) in &mut *object {
                if matches!(key.as_str(), "arms" | "goals")
                    && let Value::Object(targets) = child
                {
                    for target in targets.values_mut() {
                        convert_model_arguments_to_station_units(target)?;
                    }
                } else {
                    convert_model_arguments_to_station_units(child)?;
                }
            }
            for ModelInputUnit {
                model_field,
                station_field,
                scale_to_station,
            } in MODEL_INPUT_UNITS
            {
                if let Some(mut field) = object.remove(model_field) {
                    scale_argument(&mut field, scale_to_station)?;
                    object.insert(station_field.into(), field);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                convert_model_arguments_to_station_units(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn scale_argument(value: &mut Value, factor: f64) -> std::result::Result<(), String> {
    match value {
        Value::Array(items) => {
            for item in items {
                scale_argument(item, factor)?;
            }
        }
        Value::Number(number) => {
            let scaled = number
                .as_f64()
                .and_then(|number| serde_json::Number::from_f64(number * factor))
                .ok_or("model unit conversion requires finite numbers")?;
            *number = scaled;
        }
        _ => {}
    }
    Ok(())
}

// The station uses tagged variants while models may omit tags whose fields identify them.
fn normalize_arguments(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(at) = object.get_mut("at") {
                normalize_movement(at);
            }
            if let Some(goal) = object.get_mut("goal") {
                normalize_movement(goal);
            }
            if let Some(Value::Object(targets)) = object.get_mut("goals") {
                for target in targets.values_mut() {
                    normalize_movement(target);
                }
            }
            if let Some(Value::Object(orientation)) = object.get_mut("orientation")
                && !orientation.contains_key("kind")
            {
                let kind = if orientation.contains_key("quaternion") {
                    Some("quaternion")
                } else if orientation.contains_key("approach") {
                    Some("grasp")
                } else {
                    None
                };
                if let Some(kind) = kind {
                    orientation.insert("kind".into(), Value::String(kind.into()));
                }
            }
            for child in object.values_mut() {
                normalize_arguments(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_arguments(item);
            }
        }
        _ => {}
    }
}

fn normalize_movement(value: &mut Value) {
    if let Value::Object(goal) = value
        && !goal.contains_key("kind")
    {
        let kind = if goal.contains_key("joints") {
            Some("joints")
        } else if goal.contains_key("position") {
            Some("pose")
        } else {
            None
        };
        if let Some(kind) = kind {
            goal.insert("kind".into(), Value::String(kind.into()));
        }
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::{RobotToolOutcome, command_records_outcome};
    use hebbian_contracts::ArmRecord;

    #[test]
    fn tool_input_units_match_the_station_contract() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/model-input-units.json"))
                .unwrap();
        let mut arguments = fixture["arguments"].clone();
        super::convert_model_arguments_to_station_units(&mut arguments).unwrap();
        assert_eq!(arguments, fixture["station_command"]);
    }

    #[test]
    fn grouped_motion_distinguishes_faults_from_interruptions() {
        let examples: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/model-command-outcomes.json"
        ))
        .unwrap();
        let record_with_outcome = |outcome: &str| {
            let mut record = examples[0]["record"].clone();
            if outcome == "interrupted" {
                record["phase"] = serde_json::json!({
                    "kind": "outcome_unknown",
                    "finished_at": 2.0
                });
            } else {
                record["phase"]["completion"]["outcome"] = serde_json::json!(outcome);
            }
            ArmRecord {
                arm: outcome.into(),
                record: serde_json::from_value(record).unwrap(),
            }
        };
        assert!(matches!(
            command_records_outcome(vec![record_with_outcome("interrupted")], true).unwrap(),
            RobotToolOutcome::Interrupted(_)
        ));
        assert!(matches!(
            command_records_outcome(
                vec![
                    record_with_outcome("interrupted"),
                    record_with_outcome("faulted")
                ],
                true,
            )
            .unwrap(),
            RobotToolOutcome::Faulted(_)
        ));
    }

    #[test]
    fn group_outcomes_match_the_shared_examples() {
        let examples: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/model-command-outcomes.json"
        ))
        .unwrap();
        for example in examples {
            for grouped in [true, false] {
                let mut record_data = example["record"].clone();
                record_data["command"]["destination_joints"] = serde_json::json!([0.123_456_789]);
                record_data["command"]["joint_speed"] = serde_json::json!(0.123_456_789);
                let record: hebbian_contracts::CommandRecord =
                    serde_json::from_value(record_data.clone()).unwrap();
                assert_eq!(
                    serde_json::to_value(record.goal_status()).unwrap(),
                    example["expected_status"],
                );
                let outcome = command_records_outcome(
                    vec![ArmRecord {
                        arm: "follower".into(),
                        record,
                    }],
                    grouped,
                )
                .unwrap();
                let (value, incomplete) = match outcome {
                    RobotToolOutcome::Returned(value) => (value, false),
                    RobotToolOutcome::Incomplete(value) => (value, true),
                    _ => panic!("unexpected outcome"),
                };
                assert_eq!(incomplete, example["expected_error"]);
                let result = super::RobotToolResult {
                    cameras: std::collections::BTreeMap::new(),
                    call_id: "test-call".into(),
                    outcome: if incomplete {
                        RobotToolOutcome::Incomplete(value.clone())
                    } else {
                        RobotToolOutcome::Returned(value.clone())
                    },
                };
                let raw_record = if grouped {
                    &value["commands"][0]
                } else {
                    &value
                };
                assert_eq!(raw_record, &record_data);
                assert_eq!(result.data(), value);
                let model = result.model_data();
                let model_record = if grouped {
                    &model["commands"][0]
                } else {
                    &model
                };
                assert!(model_record.get("checkpoints").is_none());
                assert!(model_record.get("accepted_at").is_none());
                assert_eq!(
                    model_record["destination_joints_degrees"],
                    serde_json::json!([7.1])
                );
                assert_eq!(model_record["joint_speed_degrees_per_second"], 7.1);
                if grouped {
                    assert_eq!(value["status"], example["expected_status"]);
                } else {
                    assert_eq!(
                        value["phase"]["completion"]["outcome"],
                        example["record"]["phase"]["completion"]["outcome"]
                    );
                }
            }
        }
    }
}
