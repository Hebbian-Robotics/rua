//! Model-facing tool definitions derived from the station contract.
//!
//! The model supplies orientation fields without the station's `kind` tag;
//! SDK adapters add that tag before sending the request.

use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ActionSequenceArmAction, ActionSequenceStep, DEGREES_PER_RADIAN, METRES_PER_MILLIMETRE,
    MILLIMETRES_PER_METRE, MoveArmsCommand, PreviewPose, RADIANS_PER_DEGREE, StationCommand,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequenceAction {
    Pose,
    Joints,
    Gripper,
}

#[derive(Debug, Clone)]
pub enum SequenceScope {
    SingleArm,
    NamedArms(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct SequenceToolConfiguration {
    pub scope: SequenceScope,
    pub actions: BTreeSet<SequenceAction>,
}

impl Default for SequenceToolConfiguration {
    fn default() -> Self {
        Self {
            scope: SequenceScope::SingleArm,
            actions: BTreeSet::from([
                SequenceAction::Pose,
                SequenceAction::Joints,
                SequenceAction::Gripper,
            ]),
        }
    }
}

impl SequenceToolConfiguration {
    /// # Errors
    /// No action is enabled, or named arms are empty, blank, or repeated.
    pub fn validate(&self) -> Result<(), String> {
        if self.actions.is_empty() {
            return Err("A sequence needs at least one available action".into());
        }
        if let SequenceScope::NamedArms(arms) = &self.scope
            && (arms.is_empty()
                || arms.iter().any(String::is_empty)
                || arms.iter().collect::<BTreeSet<_>>().len() != arms.len())
        {
            return Err("Named-arm sequences need nonempty, unique arm names".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SingleArmActionSequenceArguments {
    #[schemars(length(min = 1))]
    pub steps: Vec<ActionSequenceArmAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewPoseArguments {
    #[schemars(length(min = 1))]
    pub poses: Vec<ModelPreviewCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPreviewCandidate {
    pub pose: PreviewPose,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceArguments {
    #[schemars(length(min = 1))]
    pub steps: Vec<ActionSequenceStep>,
}

/// Select fresh observations or retained evidence from the latest action sequence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    #[default]
    Current,
    LastSequence,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveArguments {
    #[serde(default)]
    pub source: ObservationSource,
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 10.0))]
    pub wait_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopArguments {}

/// Station operation, or local observation behavior, used by a model tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotToolOperation {
    StartActionSequence,
    Preview,
    MoveArms,
    SetGripper,
    Rehome,
    Stop,
    Observe,
}

#[must_use]
pub fn default_robot_tool_operations() -> BTreeSet<RobotToolOperation> {
    BTreeSet::from([
        RobotToolOperation::StartActionSequence,
        RobotToolOperation::Preview,
        RobotToolOperation::Stop,
        RobotToolOperation::Observe,
    ])
}

/// Typed capabilities transformed before schemas and argument permissions are built.
#[derive(Debug, Clone)]
pub enum RobotToolCapability {
    ActionSequence(SequenceToolConfiguration),
    PreviewPose,
    MoveArms,
    SetArmGripper,
    Rehome,
    Stop,
    Observe,
}

impl RobotToolCapability {
    #[must_use]
    pub fn operation(&self) -> RobotToolOperation {
        match self {
            Self::ActionSequence(_) => RobotToolOperation::StartActionSequence,
            Self::PreviewPose => RobotToolOperation::Preview,
            Self::MoveArms => RobotToolOperation::MoveArms,
            Self::SetArmGripper => RobotToolOperation::SetGripper,
            Self::Rehome => RobotToolOperation::Rehome,
            Self::Stop => RobotToolOperation::Stop,
            Self::Observe => RobotToolOperation::Observe,
        }
    }

    /// Return a capability with only the sequence actions accepted by the predicate.
    #[must_use]
    pub fn filter_sequence_actions(mut self, mut keep: impl FnMut(SequenceAction) -> bool) -> Self {
        if let Self::ActionSequence(configuration) = &mut self {
            configuration.actions.retain(|action| keep(*action));
        }
        self
    }
}

/// All independently selectable capabilities, including standalone operations.
#[must_use]
pub fn all_robot_tool_capabilities() -> Vec<RobotToolCapability> {
    vec![
        RobotToolCapability::ActionSequence(SequenceToolConfiguration::default()),
        RobotToolCapability::PreviewPose,
        RobotToolCapability::MoveArms,
        RobotToolCapability::SetArmGripper,
        RobotToolCapability::Rehome,
        RobotToolCapability::Stop,
        RobotToolCapability::Observe,
    ]
}

#[must_use]
pub fn default_robot_tool_mapping(
    capabilities: Vec<RobotToolCapability>,
) -> Vec<RobotToolCapability> {
    let operations = default_robot_tool_operations();
    capabilities
        .into_iter()
        .filter(|tool| operations.contains(&tool.operation()))
        .collect()
}

pub fn filter_sequence_actions(
    mut keep: impl FnMut(SequenceAction) -> bool,
) -> impl FnOnce(Vec<RobotToolCapability>) -> Vec<RobotToolCapability> {
    move |capabilities| {
        capabilities
            .into_iter()
            .map(|tool| tool.filter_sequence_actions(&mut keep))
            .collect()
    }
}

/// Model-facing classification shared by command and sequence adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOutcome {
    Completed,
    StoppedShort,
    Faulted,
    Interrupted,
    Incomplete,
}

impl From<crate::CommandOutcome> for ModelOutcome {
    fn from(outcome: crate::CommandOutcome) -> Self {
        use crate::CommandOutcome;
        match outcome {
            CommandOutcome::Completed => Self::Completed,
            CommandOutcome::StoppedShort => Self::StoppedShort,
            CommandOutcome::Faulted => Self::Faulted,
            CommandOutcome::Interrupted => Self::Interrupted,
            CommandOutcome::Stopped
            | CommandOutcome::Tripped
            | CommandOutcome::TimedOut
            | CommandOutcome::Superseded
            | CommandOutcome::Failed => Self::Incomplete,
        }
    }
}

/// Lowest to highest priority when combining command records into a group result.
pub const MODEL_GROUP_STATUSES: [crate::GoalStatus; 4] = [
    crate::GoalStatus::Reached,
    crate::GoalStatus::Limited,
    crate::GoalStatus::Partial,
    crate::GoalStatus::StoppedShort,
];

impl ModelOutcome {
    #[must_use]
    pub fn goal_status(self, limited: bool) -> crate::GoalStatus {
        use crate::GoalStatus;
        match self {
            Self::Completed if limited => GoalStatus::Limited,
            Self::Completed => GoalStatus::Reached,
            Self::StoppedShort => GoalStatus::StoppedShort,
            Self::Incomplete | Self::Faulted | Self::Interrupted => GoalStatus::Partial,
        }
    }
}

/// Fields selected by the concise model response formatter.
#[derive(Debug, Serialize)]
pub struct ModelResponseFields {
    pub state: &'static [&'static str],
    pub station_context: &'static [&'static str],
    pub arm: &'static [&'static str],
    pub diagnostic: &'static [&'static str],
    pub earlier_step: &'static [&'static str],
}

/// Shared field selections for station snapshots and command history.
pub const MODEL_RESPONSE_FIELDS: ModelResponseFields = ModelResponseFields {
    state: &["robot", "joint_names", "max_joint_speed", "surface_guard"],
    station_context: &["description", "lease"],
    arm: &[
        "arm",
        "shoulder_pan_axis_position_meters",
        "mode",
        "fault",
        "active_trip",
        "interlocks",
        "recovery_issue",
        "revision",
        "sampled_unix_nano",
        "measured_joints",
        "tool",
        "gripper_opening",
    ],
    diagnostic: &[
        "id",
        "command_id",
        "group_id",
        "stream_command_id",
        "sequence_id",
        "action_sequence_id",
        "checkpoints",
        "trace_context",
        "model_points",
        "accepted_unix_nano",
        "finished_unix_nano",
    ],
    earlier_step: &[
        "label",
        "completion",
        "phase",
        "goal_status",
        "pose_limited",
        "path_limited",
    ],
};

/// Station field, model field, and multiplier for model-facing output.
pub const MODEL_OUTPUT_UNITS: &[(&str, &str, f64)] = &[
    (
        "shoulder_pan_axis_position_meters",
        "shoulder_pan_axis_position_mm",
        MILLIMETRES_PER_METRE,
    ),
    ("position", "position_mm", MILLIMETRES_PER_METRE),
    ("goal_position", "goal_position_mm", MILLIMETRES_PER_METRE),
    (
        "destination_position",
        "destination_position_mm",
        MILLIMETRES_PER_METRE,
    ),
    ("tool_position", "tool_position_mm", MILLIMETRES_PER_METRE),
    (
        "surface_height_meters",
        "surface_height_mm",
        MILLIMETRES_PER_METRE,
    ),
    ("leeway_meters", "leeway_mm", MILLIMETRES_PER_METRE),
    ("model_points", "model_points_mm", MILLIMETRES_PER_METRE),
    (
        "position_error_meters",
        "position_error_mm",
        MILLIMETRES_PER_METRE,
    ),
    (
        "position_shortfall_meters",
        "position_shortfall_mm",
        MILLIMETRES_PER_METRE,
    ),
    ("jaw_yaw_radians", "jaw_yaw_degrees", DEGREES_PER_RADIAN),
    ("radians_from_down", "degrees_from_down", DEGREES_PER_RADIAN),
    (
        "approach_error_radians",
        "approach_error_degrees",
        DEGREES_PER_RADIAN,
    ),
    ("from_radians", "from_degrees", DEGREES_PER_RADIAN),
    ("to_radians", "to_degrees", DEGREES_PER_RADIAN),
    ("joints", "joints_degrees", DEGREES_PER_RADIAN),
    (
        "measured_joints",
        "measured_joints_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "measured_joint_velocities",
        "measured_joint_velocities_degrees_per_second",
        DEGREES_PER_RADIAN,
    ),
    (
        "commanded_joints",
        "commanded_joints_degrees",
        DEGREES_PER_RADIAN,
    ),
    ("rest_pose", "rest_pose_degrees", DEGREES_PER_RADIAN),
    (
        "destination_joints",
        "destination_joints_degrees",
        DEGREES_PER_RADIAN,
    ),
    ("goal_joints", "goal_joints_degrees", DEGREES_PER_RADIAN),
    (
        "joint_error_radians",
        "joint_error_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "joint_errors_radians",
        "joint_errors_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "orientation_error_radians",
        "orientation_error_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "orientation_shortfall_radians",
        "orientation_shortfall_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "measured_angle_radians",
        "measured_angle_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "target_angle_radians",
        "target_angle_degrees",
        DEGREES_PER_RADIAN,
    ),
    (
        "joint_speed",
        "joint_speed_degrees_per_second",
        DEGREES_PER_RADIAN,
    ),
    (
        "max_joint_speed",
        "max_joint_speed_degrees_per_second",
        DEGREES_PER_RADIAN,
    ),
    (
        "max_joint_acceleration",
        "max_joint_acceleration_degrees_per_second_squared",
        DEGREES_PER_RADIAN,
    ),
    (
        "max_joint_jerk",
        "max_joint_jerk_degrees_per_second_cubed",
        DEGREES_PER_RADIAN,
    ),
];

/// A numeric tool argument whose model units differ from the station contract.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelInputUnit {
    pub model_field: &'static str,
    pub station_field: &'static str,
    pub scale_to_station: f64,
}

/// Unit conversions shared by the model tool schema and all SDK adapters.
pub const MODEL_INPUT_UNITS: [ModelInputUnit; 6] = [
    ModelInputUnit {
        model_field: "position_mm",
        station_field: "position",
        scale_to_station: METRES_PER_MILLIMETRE,
    },
    ModelInputUnit {
        model_field: "jaw_yaw_degrees",
        station_field: "jaw_yaw_radians",
        scale_to_station: RADIANS_PER_DEGREE,
    },
    ModelInputUnit {
        model_field: "joints_degrees",
        station_field: "joints",
        scale_to_station: RADIANS_PER_DEGREE,
    },
    ModelInputUnit {
        model_field: "max_joint_speed_degrees_per_second",
        station_field: "max_joint_speed",
        scale_to_station: RADIANS_PER_DEGREE,
    },
    ModelInputUnit {
        model_field: "max_joint_acceleration_degrees_per_second_squared",
        station_field: "max_joint_acceleration",
        scale_to_station: RADIANS_PER_DEGREE,
    },
    ModelInputUnit {
        model_field: "max_joint_jerk_degrees_per_second_cubed",
        station_field: "max_joint_jerk",
        scale_to_station: RADIANS_PER_DEGREE,
    },
];

/// A provider-independent model tool specification.
#[derive(Debug, Clone, Serialize)]
pub struct RobotToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
    pub operation: RobotToolOperation,
}

fn model_arguments<T: JsonSchema>() -> Value {
    adapt_model_schema(schema_for!(T))
}

fn adapt_model_schema(schema: Schema) -> Value {
    let mut schema = schema.to_value();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    adapt(&mut schema);
    schema
}

// CONSTRAINT: SDK adapters infer variant tags and convert explicit model units to SI.
fn adapt(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("format");
            object.remove("x-omit-when-none");
            // CONSTRAINT: Use the broadly supported anyOf spelling for nullable values.
            if let Some(Value::Array(types)) = object.get("type")
                && types.len() == 2
                && types.iter().any(|kind| kind == "null")
                && let Some(non_null) = types.iter().find(|kind| *kind != "null").cloned()
            {
                object.remove("type");
                object.insert(
                    "anyOf".into(),
                    json!([{ "type": non_null }, { "type": "null" }]),
                );
            }
            if object.get("type") == Some(&json!("object"))
                && !object.contains_key("properties")
                && object.get("additionalProperties") == Some(&json!(false))
            {
                object.insert("properties".into(), json!({}));
            }
            if let Some(Value::Object(properties)) = object.get_mut("properties")
                && let Some(Value::Object(kind)) = properties.get_mut("kind")
                && let Some(tag) = kind.get("const").and_then(Value::as_str)
                && matches!(tag, "grasp" | "quaternion" | "joints" | "pose")
            {
                properties.remove("kind");
                if let Some(Value::Array(required)) = object.get_mut("required") {
                    required.retain(|field| field != "kind");
                }
            }
            for ModelInputUnit {
                station_field,
                model_field,
                ..
            } in MODEL_INPUT_UNITS
            {
                if let Some(Value::Object(properties)) = object.get_mut("properties")
                    && let Some(mut property) = properties.remove(station_field)
                {
                    let description = match station_field {
                        "jaw_yaw_radians" => Some("Jaw yaw in degrees."),
                        "max_joint_speed" => Some(
                            "Peak speed in degrees per second; clamped to the profile maximum.",
                        ),
                        "max_joint_acceleration" => Some(
                            "Joint acceleration cap in degrees per second squared; defaults to the arm profile.",
                        ),
                        "max_joint_jerk" => Some(
                            "Joint jerk cap in degrees per second cubed; defaults to the arm profile.",
                        ),
                        _ => None,
                    };
                    if let Some(description) = description
                        && let Ok(schema) = <&mut Schema>::try_from(&mut property)
                    {
                        schema.insert("description".into(), json!(description));
                    }
                    properties.insert(model_field.into(), property);
                    if let Some(Value::Array(required)) = object.get_mut("required") {
                        for field in required {
                            if field == station_field {
                                *field = json!(model_field);
                            }
                        }
                    }
                }
            }
            for child in object.values_mut() {
                adapt(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                adapt(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn schema_variant<'a>(schema: &'a Value, keyword: &str, title: &str) -> Result<&'a Value, String> {
    schema[keyword]
        .as_array()
        .and_then(|variants| variants.iter().find(|variant| variant["title"] == title))
        .ok_or_else(|| format!("Missing {title} in generated {keyword} schema"))
}

fn gripper_arguments_schema() -> Result<Value, String> {
    #[derive(Serialize, Deserialize)]
    struct CommandArguments {
        properties: serde_json::Map<String, Value>,
        required: Vec<String>,
        #[serde(flatten)]
        keywords: serde_json::Map<String, Value>,
    }

    let command = model_arguments::<StationCommand>();
    let variant = schema_variant(&command, "oneOf", "StationSetGripperCommand")?;
    let mut arguments: CommandArguments = serde_json::from_value(variant.clone())
        .map_err(|error| format!("Invalid gripper command schema: {error}"))?;
    arguments.keywords.remove("title");
    arguments.properties.remove("kind");
    arguments
        .properties
        .insert("arm".into(), json!({"type": "string"}));
    arguments.required.retain(|name| name != "kind");
    arguments.required.push("arm".into());
    Ok(json!(arguments))
}

/// The explicit set of robot capabilities exposed to model frameworks.
///
/// # Panics
/// Panics if the built-in default sequence or generated command schemas are inconsistent.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "The default sequence configuration is valid and covered by the catalog contract tests."
)]
pub fn robot_tool_catalog() -> Vec<RobotToolDefinition> {
    all_robot_tool_capabilities()
        .into_iter()
        .map(|capability| {
            capability
                .definition()
                .expect("default sequence configuration")
        })
        .collect()
}

fn action_sequence_description(configuration: &SequenceToolConfiguration) -> String {
    let has_motion = configuration.actions.contains(&SequenceAction::Pose)
        || configuration.actions.contains(&SequenceAction::Joints);
    let has_gripper = configuration.actions.contains(&SequenceAction::Gripper);
    let mut description = String::from("Run steps in order, waiting for each to finish. ");
    if has_motion {
        description.push_str("A move passes through waypoints and settles at its goal. Waypoints must pass through without stopping; make a required stop a goal in a separate step. ");
        if matches!(configuration.scope, SequenceScope::NamedArms(_)) {
            description.push_str(
                "Waypoints, timing, and per-move acceleration or jerk require a single-arm step. ",
            );
        }
        if has_gripper {
            description.push_str("A waypoint gripper_opening starts at that crossing. ");
        }
        description.push_str("A refusal, incomplete move, or limited goal ends the sequence; earlier steps may already have run.");
    } else {
        description.push_str("A refusal or incomplete action ends the sequence; earlier steps may already have run. Contact does not confirm a grasp.");
    }
    description
}

impl RobotToolCapability {
    /// The model-facing name, independent of the arguments schema.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ActionSequence(_) => "action_sequence",
            Self::PreviewPose => "preview_pose",
            Self::MoveArms => "move_arms",
            Self::SetArmGripper => "set_arm_gripper",
            Self::Rehome => "rehome",
            Self::Stop => "stop",
            Self::Observe => "observe",
        }
    }

    /// Build the model-facing definition with the selected sequence configuration.
    ///
    /// # Errors
    /// Returns an error for invalid sequence configuration or missing station schema fields.
    pub fn definition(&self) -> Result<RobotToolDefinition, String> {
        let (description, parameters) = match self {
            Self::ActionSequence(configuration) => (
                action_sequence_description(configuration),
                action_sequence_tool_schema(configuration)?,
            ),
            Self::PreviewPose => (
                "Resolve pose feasibility without moving: returns projected poses, errors, available jaw yaws or refusal in input order. within_tolerance is a preview result, not a command input. Omitting orientation lets the solver choose one. Point previews do not validate the sequence path or clearance from workspace objects.".into(),
                model_arguments::<PreviewPoseArguments>(),
            ),
            Self::MoveArms => (
                "Move named arms together. Pose goals use a world position and orientation; joint goals use degrees. If one arm fails during motion, the others stop. Grouped moves check modeled inter-arm clearance, not workspace objects.".into(),
                model_arguments::<MoveArmsCommand>(),
            ),
            Self::SetArmGripper => (
                "Set a named arm's gripper opening from 0 (closed) to 1 (fully open). Contact may stop closing but does not confirm a grasp.".into(),
                gripper_arguments_schema()?,
            ),
            Self::Rehome => (
                "Return this arm to the joint pose and gripper opening recorded when it was enabled.".into(),
                model_arguments::<StopArguments>(),
            ),
            Self::Stop => (
                "Stop this arm's motion and hold position; this does not disable the motors or end the agent run.".into(),
                model_arguments::<StopArguments>(),
            ),
            Self::Observe => (
                "Observe without moving. source=current (default) gets fresh state and camera images; source=last_sequence retrieves sampled images from the latest action sequence to investigate what happened during motion. History is only returned when requested and may be unavailable or incomplete. Optionally wait up to 10 seconds before observing.".into(),
                model_arguments::<ObserveArguments>(),
            ),
        };
        Ok(RobotToolDefinition {
            name: self.name(),
            description,
            parameters,
            operation: self.operation(),
        })
    }
}

/// Robot conventions and limitations independent of a particular station.
#[must_use]
pub fn robot_tool_context() -> Value {
    json!({
        "frames": "Positions are in the station world frame. Tool inputs and outputs use millimeters, degrees, and degrees per second.",
        "orientation": format!("Approach points from palm toward fingertips: down (world -Z), forward (world +X), or a world vector. Jaw yaw describes the closing axis about minus approach; for down, 0 degrees closes along world X and 90 along world Y. Zero uses projected world +X, switching to +Y when the absolute normalized approach X exceeds {}. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.", crate::JAW_REFERENCE_SWITCH_APPROACH_X),
        "motion": "A move passes through optional waypoints in order and settles at its goal. A waypoint gripper_opening starts at that waypoint. For one planned arm per step, optional at_seconds sets an earliest reference arrival from the start of that step. Time every waypoint and the goal in strictly increasing order. Motion limits can make each section take longer, and the next step starts after settling. A pose goal is solved to joints; the arm follows a joint-space path, not necessarily a straight Cartesian line. Use separate approach, descend and lift steps when clearance requires them. Tool position is the configured tool-frame origin, not the lowest point of the fingers or camera.",
        "surface_guard": "The configured surface height is a nominal or measured table plane. Leeway permits modeled geometry below that plane; it does not lower the table. The guard checks the lowest moving geometry, not just the tool frame, and permits recovery upward from below its limit.",
        "gripper": "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. The step speed applies to the arm and gripper and overrides the speed in the goal.",
        "outcomes": "A completed move with goal_status limited finished at an adjusted target; action sequences stop on pose or path limits. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are refused.",
        "limitations": "Model guards can check self-collision and a horizontal surface. Grouped moves also check modeled clearance among participating arms. These checks do not detect workspace objects or establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified."
    })
}

#[expect(
    clippy::expect_used,
    reason = "Schema references are local to the generated contract."
)]
fn inline_schema(value: &Value, definitions: &Value) -> Value {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let name = reference
                    .strip_prefix("#/$defs/")
                    .expect("local schema reference");
                return inline_schema(&definitions[name], definitions);
            }
            Value::Object(
                object
                    .iter()
                    .filter(|(key, _)| key.as_str() != "$defs")
                    .map(|(key, value)| (key.clone(), inline_schema(value, definitions)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| inline_schema(item, definitions))
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
    }
}
#[expect(
    clippy::expect_used,
    reason = "Pose and joint schemas define the two goal variants."
)]
fn select_goal_variants(goal: &mut Value, configuration: &SequenceToolConfiguration) {
    let mut variants = goal["oneOf"].as_array().expect("goal variants").clone();
    variants.retain(|variant| {
        let capability = if variant["properties"].get("joints_degrees").is_some() {
            SequenceAction::Joints
        } else {
            SequenceAction::Pose
        };
        configuration.actions.contains(&capability)
    });
    *goal = if variants.len() == 1 {
        variants.remove(0)
    } else {
        json!({"oneOf": variants})
    };
}

/// Compose the model's sequence schema without exposing unused actions or arm selectors.
///
/// # Errors
/// The configuration is invalid or the generated station schema lacks a selected action.
pub fn action_sequence_tool_schema(
    configuration: &SequenceToolConfiguration,
) -> Result<Value, String> {
    configuration.validate()?;
    let source = model_arguments::<ActionSequenceArmAction>();
    let variants = inline_schema(&source, &source["$defs"]);
    let mut action = variants["anyOf"]
        .as_array()
        .ok_or("arm action variants")?
        .iter()
        .find(|variant| variant["properties"].get("goal").is_some())
        .ok_or("move action")?
        .clone();
    let action_fields = action.as_object_mut().ok_or("move action schema")?;
    action_fields.remove("required");
    action_fields.remove("title");
    action_fields.insert("description".into(), source["description"].clone());
    let properties = action["properties"]
        .as_object_mut()
        .ok_or("action properties")?;
    if configuration.actions.contains(&SequenceAction::Pose)
        || configuration.actions.contains(&SequenceAction::Joints)
    {
        let goal = properties.get_mut("goal").ok_or("goal schema")?;
        select_goal_variants(goal, configuration);
    } else {
        properties.remove("goal");
        properties.remove("waypoints");
        properties.remove("at_seconds");
    }
    if let Some(points) = properties.get_mut("waypoints") {
        let fields = points["items"]["properties"]
            .as_object_mut()
            .ok_or("waypoint fields")?;
        select_goal_variants(
            fields.get_mut("at").ok_or("waypoint configuration")?,
            configuration,
        );
        if !configuration.actions.contains(&SequenceAction::Gripper) {
            fields.remove("gripper_opening");
        }
    }
    if configuration.actions.contains(&SequenceAction::Gripper) {
        let opening = properties
            .get_mut("gripper_opening")
            .ok_or("opening schema")?;
        if let Some(opening) = opening.as_object_mut() {
            opening.remove("anyOf");
            opening.remove("default");
            opening.insert("type".into(), json!("number"));
        }
    } else {
        properties.remove("gripper_opening");
    }
    let required_actions: Vec<_> = [("goal", "object"), ("gripper_opening", "number")]
        .into_iter()
        .filter(|(name, _)| properties.contains_key(*name))
        .map(|(name, kind)| {
            json!({
                "required": [name],
                "properties": {name: {"type": kind}},
            })
        })
        .collect();
    action["anyOf"] = json!(required_actions);
    let step = match &configuration.scope {
        SequenceScope::SingleArm => action,
        SequenceScope::NamedArms(arms) => json!({
            "type": "object", "additionalProperties": false, "required": ["arms"],
            "properties": {"arms": {"type": "object", "minProperties": 1, "additionalProperties": false,
                "properties": arms.iter().map(|arm| (arm.clone(), action.clone())).collect::<serde_json::Map<_,_>>()}}
        }),
    };
    Ok(
        json!({"type": "object", "additionalProperties": false, "required": ["steps"],
        "properties": {"steps": {"type": "array", "minItems": 1, "items": step}}}),
    )
}

/// Parse a specialized interface into the general station step representation.
///
/// # Errors
/// The arguments use an unavailable arm/action, omit an action, or are malformed.
pub fn parse_sequence_tool_arguments(
    arguments: Value,
    configuration: &SequenceToolConfiguration,
    selected_arm: &str,
) -> Result<Vec<ActionSequenceStep>, String> {
    configuration.validate()?;
    let steps = match &configuration.scope {
        SequenceScope::SingleArm => {
            serde_json::from_value::<SingleArmActionSequenceArguments>(arguments)
                .map_err(|error| error.to_string())?
                .steps
                .into_iter()
                .map(|action| ActionSequenceStep::single(selected_arm, action))
                .collect::<Vec<_>>()
        }
        SequenceScope::NamedArms(_) => {
            serde_json::from_value::<ActionSequenceArguments>(arguments)
                .map_err(|error| error.to_string())?
                .steps
        }
    };
    if steps.is_empty() {
        return Err("An action sequence needs at least one step".into());
    }
    for step in &steps {
        if step.arms.is_empty() {
            return Err("A step needs at least one arm".into());
        }
        for (arm, action) in &step.arms {
            if let SequenceScope::NamedArms(arms) = &configuration.scope
                && !arms.contains(arm)
            {
                return Err(format!("Arm {arm} is not available in this sequence"));
            }
            if let crate::ActionSequenceArmAction::Move { waypoints, .. } = action {
                for point in waypoints {
                    let capability = match point.at {
                        crate::CompiledGoal::Joints { .. } => SequenceAction::Joints,
                        crate::CompiledGoal::Pose { .. } => SequenceAction::Pose,
                    };
                    if !configuration.actions.contains(&capability)
                        || (point.gripper_opening.is_some()
                            && !configuration.actions.contains(&SequenceAction::Gripper))
                    {
                        return Err(
                            "This waypoint action is not available in the configured sequence"
                                .into(),
                        );
                    }
                }
            }
            let movement_capability = action.goal().map(|goal| match goal {
                crate::ActionSequenceGoal::Pose { .. } => SequenceAction::Pose,
                crate::ActionSequenceGoal::Joints { .. } => SequenceAction::Joints,
            });
            if movement_capability
                .is_some_and(|capability| !configuration.actions.contains(&capability))
                || (action.gripper_opening().is_some()
                    && !configuration.actions.contains(&SequenceAction::Gripper))
            {
                return Err("This action is not available in the configured sequence".into());
            }
        }
    }
    Ok(steps)
}

#[cfg(test)]
mod sequence_configuration_tests {
    use super::*;

    #[test]
    fn sequence_description_matches_available_actions_and_scope() {
        let single = RobotToolCapability::ActionSequence(SequenceToolConfiguration {
            scope: SequenceScope::SingleArm,
            actions: BTreeSet::from([SequenceAction::Pose]),
        })
        .definition()
        .unwrap();
        assert!(single.description.contains("without stopping"));
        assert!(!single.description.contains("gripper_opening"));
        assert!(!single.description.contains("single-arm step"));

        let named = RobotToolCapability::ActionSequence(SequenceToolConfiguration {
            scope: SequenceScope::NamedArms(vec!["left".into(), "right".into()]),
            actions: BTreeSet::from([SequenceAction::Joints, SequenceAction::Gripper]),
        })
        .definition()
        .unwrap();
        assert!(named.description.contains("single-arm step"));
        assert!(named.description.contains("gripper_opening"));

        let gripper_only = RobotToolCapability::ActionSequence(SequenceToolConfiguration {
            scope: SequenceScope::SingleArm,
            actions: BTreeSet::from([SequenceAction::Gripper]),
        })
        .definition()
        .unwrap();
        assert!(!gripper_only.description.contains("waypoint"));
        assert!(
            gripper_only
                .description
                .contains("does not confirm a grasp")
        );
    }

    #[test]
    fn the_wire_rejects_an_arm_action_without_a_target() {
        for action in [
            json!({}),
            json!({"max_joint_speed": 0.5}),
            json!({"gripper_opening": null}),
        ] {
            assert!(serde_json::from_value::<ActionSequenceArmAction>(action).is_err());
        }
        assert!(
            serde_json::from_value::<ActionSequenceArmAction>(json!({"gripper_opening": 0.5}))
                .is_ok()
        );
    }

    #[test]
    fn a_single_arm_pose_gripper_interface_has_no_joint_or_arm_choices_and_rejects_them() {
        let configuration = SequenceToolConfiguration {
            scope: SequenceScope::SingleArm,
            actions: BTreeSet::from([SequenceAction::Pose, SequenceAction::Gripper]),
        };
        let schema = action_sequence_tool_schema(&configuration).unwrap();
        let fields = &schema["properties"]["steps"]["items"]["properties"];
        assert!(fields.get("arms").is_none());
        assert!(fields["goal"]["properties"].get("joints_degrees").is_none());
        assert!(fields["goal"]["properties"].get("position_mm").is_some());
        assert!(fields.get("gripper_opening").is_some());
        let steps = parse_sequence_tool_arguments(
            json!({"steps": [{"gripper_opening": 0.5}]}),
            &configuration,
            "follower",
        )
        .unwrap();
        assert_eq!(steps[0].arms["follower"].gripper_opening(), Some(0.5));
        for unavailable in [
            json!({"steps": [{"goal": {"kind": "joints", "joints": [0.0]}}]}),
            json!({"steps": [{"arms": {"right": {"gripper_opening": 0.5}}}]}),
            json!({"steps": [{}]}),
        ] {
            assert!(
                parse_sequence_tool_arguments(unavailable, &configuration, "follower").is_err()
            );
        }
    }

    #[test]
    fn named_arm_gripper_sequences_select_only_configured_arms_and_capabilities() {
        let configuration = SequenceToolConfiguration {
            scope: SequenceScope::NamedArms(vec!["left".into(), "right".into()]),
            actions: BTreeSet::from([SequenceAction::Gripper]),
        };
        let schema = action_sequence_tool_schema(&configuration).unwrap();
        let arm_fields =
            &schema["properties"]["steps"]["items"]["properties"]["arms"]["properties"];
        assert!(arm_fields["left"]["properties"].get("goal").is_none());
        let steps = parse_sequence_tool_arguments(
            json!({"steps": [{"arms": {
                "left": {"gripper_opening": 0.2}, "right": {"gripper_opening": 0.8}
            }}]}),
            &configuration,
            "left",
        )
        .unwrap();
        assert_eq!(steps[0].arms.len(), 2);
        assert_eq!(steps[0].arms["right"].gripper_opening(), Some(0.8));
        assert!(
            parse_sequence_tool_arguments(
                json!({"steps": [{"arms": {"other": {"gripper_opening": 0.5}}}]}),
                &configuration,
                "left"
            )
            .is_err()
        );
        assert!(
            SequenceToolConfiguration {
                scope: SequenceScope::NamedArms(vec![]),
                ..configuration
            }
            .validate()
            .is_err()
        );
    }
}
