//! Model-facing tool definitions derived from the station contract.
//!
//! The model supplies orientation fields without the station's `kind` tag;
//! SDK adapters add that tag before sending the request.

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ActionSequenceArmAction, ActionSequenceStep, MoveArmsCommand, PreviewPose, StationCommand,
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
        "mode",
        "status",
        "fault",
        "active_trip",
        "interlocks",
        "recovery_issue",
        "revision",
        "sampled_at_unix_seconds",
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
        "accepted_at",
        "finished_at",
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
    ("position", "position_mm", 1000.0),
    ("goal_position", "goal_position_mm", 1000.0),
    ("destination_position", "destination_position_mm", 1000.0),
    ("tool_position", "tool_position_mm", 1000.0),
    ("surface_height_meters", "surface_height_mm", 1000.0),
    ("leeway_meters", "leeway_mm", 1000.0),
    ("model_points", "model_points_mm", 1000.0),
    ("position_error_meters", "position_error_mm", 1000.0),
    ("position_shortfall_meters", "position_shortfall_mm", 1000.0),
    ("joints", "joints_degrees", 180.0 / std::f64::consts::PI),
    (
        "measured_joints",
        "measured_joints_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "measured_joint_velocities",
        "measured_joint_velocities_degrees_per_second",
        180.0 / std::f64::consts::PI,
    ),
    (
        "commanded_joints",
        "commanded_joints_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "rest_pose",
        "rest_pose_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "destination_joints",
        "destination_joints_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "goal_joints",
        "goal_joints_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "joint_error_radians",
        "joint_error_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "joint_errors_radians",
        "joint_errors_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "orientation_error_radians",
        "orientation_error_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "orientation_shortfall_radians",
        "orientation_shortfall_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "measured_angle_radians",
        "measured_angle_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "target_angle_radians",
        "target_angle_degrees",
        180.0 / std::f64::consts::PI,
    ),
    (
        "joint_speed",
        "joint_speed_degrees_per_second",
        180.0 / std::f64::consts::PI,
    ),
    (
        "max_joint_speed",
        "max_joint_speed_degrees_per_second",
        180.0 / std::f64::consts::PI,
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
pub const MODEL_INPUT_UNITS: [ModelInputUnit; 3] = [
    ModelInputUnit {
        model_field: "position_mm",
        station_field: "position",
        scale_to_station: 0.001,
    },
    ModelInputUnit {
        model_field: "joints_degrees",
        station_field: "joints",
        scale_to_station: std::f64::consts::PI / 180.0,
    },
    ModelInputUnit {
        model_field: "max_joint_speed_degrees_per_second",
        station_field: "max_joint_speed",
        scale_to_station: std::f64::consts::PI / 180.0,
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

#[expect(
    clippy::expect_used,
    reason = "Schemars schemas contain only JSON-compatible data and cannot fail JSON serialization."
)]
fn model_arguments<T: JsonSchema>() -> Value {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("schema is serializable");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    adapt(&mut schema);
    schema
}

// SDK adapters infer variant tags and convert explicit model units to SI.
#[expect(
    clippy::expect_used,
    reason = "The speed property is replaced with an object schema immediately before it is accessed."
)]
fn adapt(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("format");
            object.remove("x-omit-when-none");
            // Use the broadly supported anyOf spelling for nullable values.
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
                    if station_field == "max_joint_speed" {
                        property.as_object_mut().expect("speed schema").insert(
                            "description".into(),
                            json!(
                                "Peak speed in degrees per second; clamped to the profile maximum."
                            ),
                        );
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

// The gripper tool adds an arm selector to the station's command fields.
#[expect(
    clippy::expect_used,
    reason = "The generated Command schema defines SetGripper and its required fields; contract tests guard the shape."
)]
fn gripper_arguments_schema() -> Value {
    let command_schema = model_arguments::<StationCommand>();
    let mut schema = command_schema["oneOf"]
        .as_array()
        .and_then(|variants| {
            variants
                .iter()
                .find(|variant| variant["title"] == "StationSetGripperCommand")
        })
        .expect("the station contract has a gripper command")
        .clone();
    let properties = schema["properties"]
        .as_object_mut()
        .expect("gripper fields");
    properties.remove("kind");
    properties.insert("arm".into(), json!({"type": "string"}));
    let required = schema["required"]
        .as_array_mut()
        .expect("gripper required fields");
    required.retain(|field| field != "kind");
    required.push(json!("arm"));
    schema
        .as_object_mut()
        .expect("gripper schema")
        .remove("title");
    schema
}

/// The explicit set of robot capabilities exposed to model frameworks.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "The default sequence configuration is valid and covered by the catalog contract tests."
)]
pub fn robot_tool_catalog() -> Vec<RobotToolDefinition> {
    vec![
        RobotToolDefinition {
            name: "action_sequence",
            operation: RobotToolOperation::StartActionSequence,
            description: "Run steps in order, waiting for each to finish. During a move, pass through optional waypoints and settle at the goal. A waypoint gripper_opening starts at that crossing. A refusal, incomplete move, or limited goal ends the sequence.".into(),
            parameters: action_sequence_tool_schema(&SequenceToolConfiguration::default()).expect("default sequence configuration"),
        },
        RobotToolDefinition {
            name: "preview_pose",
            operation: RobotToolOperation::Preview,
            description: "Resolve pose feasibility without moving: returns projected poses, errors, available jaw yaws or refusal in input order. Execution also checks feasibility; preview is useful when the projection or orientation choice changes your next action. Omitting orientation lets the solver choose one.".into(),
            parameters: model_arguments::<PreviewPoseArguments>(),
        },
        RobotToolDefinition {
            name: "move_arms",
            operation: RobotToolOperation::MoveArms,
            description: "Move named arms together. Pose goals use a world position and orientation; joint goals use degrees. Grouped moves check modeled inter-arm clearance. Workspace objects require separate constraints.".into(),
            parameters: model_arguments::<MoveArmsCommand>(),
        },
        RobotToolDefinition {
            name: "set_arm_gripper",
            operation: RobotToolOperation::SetGripper,
            description: "Set a named arm's gripper opening from 0 (closed) to 1 (fully open); intermediate values request partial openings.".into(),
            parameters: gripper_arguments_schema(),
        },
        RobotToolDefinition {
            name: "rehome",
            operation: RobotToolOperation::Rehome,
            description: "Return this arm to the joint pose recorded when it was enabled. The gripper keeps its current target.".into(),
            parameters: model_arguments::<StopArguments>(),
        },
        RobotToolDefinition {
            name: "stop",
            operation: RobotToolOperation::Stop,
            description: "Stop this arm's motion; it holds where it stops.".into(),
            parameters: model_arguments::<StopArguments>(),
        },
        RobotToolDefinition {
            name: "observe",
            operation: RobotToolOperation::Observe,
            description: "Observe without moving. source=current (default) gets fresh state and camera images; source=last_sequence retrieves sampled images from the latest action sequence to investigate what happened during motion. History is only returned when requested and may be unavailable or incomplete. Optionally wait up to 10 seconds before observing.".into(),
            parameters: model_arguments::<ObserveArguments>(),
        },
    ]
}

/// Robot conventions and limitations independent of a particular station.
#[must_use]
pub fn robot_tool_context() -> Value {
    json!({
        "frames": "Positions are in the station world frame. Tool inputs and outputs use millimeters, degrees, and degrees per second.",
        "orientation": format!("Approach points from palm toward fingertips: down (world -Z), forward (world +X), or a world vector. Jaw yaw describes the closing axis about minus approach; for down, 0 degrees closes along world X and 90 along world Y. Zero uses projected world +X, switching to +Y when the absolute normalized approach X exceeds {}. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.", crate::JAW_REFERENCE_SWITCH_APPROACH_X),
        "motion": "A move passes through optional waypoints in order and settles at its goal. A waypoint gripper_opening starts at that waypoint. A pose goal is solved to joints; the arm follows a joint-space path, not necessarily a straight Cartesian line. Use separate approach, descend and lift steps when clearance requires them. Tool position is the configured tool-frame origin, not the lowest point of the fingers or camera.",
        "surface_guard": "The configured surface height is a nominal or measured table plane. Leeway permits modeled geometry below that plane; it does not lower the table. The guard checks the lowest moving geometry, not just the tool frame, and permits recovery upward from below its limit.",
        "gripper": "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. The step speed applies to the arm and gripper and overrides the speed in the goal.",
        "outcomes": "A completed move with goal_status limited finished at an adjusted target; action sequences stop on pose or path limits. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are refused.",
        "limitations": "Model guards can check self-collision and a horizontal surface, but do not detect workspace objects or other arms and do not establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified."
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
/// The configuration enables no actions or contains invalid arm names.
#[expect(
    clippy::expect_used,
    reason = "Local references and action fields come from our generated schema; contract tests guard its shape."
)]
pub fn action_sequence_tool_schema(
    configuration: &SequenceToolConfiguration,
) -> Result<Value, String> {
    configuration.validate()?;
    let source = model_arguments::<ActionSequenceArmAction>();
    let variants = inline_schema(&source, &source["$defs"]);
    let mut action = variants["anyOf"]
        .as_array()
        .expect("arm action variants")
        .iter()
        .find(|variant| variant["properties"].get("goal").is_some())
        .expect("move action")
        .clone();
    let action_fields = action.as_object_mut().expect("move action schema");
    action_fields.remove("required");
    action_fields.remove("title");
    action_fields.insert("description".into(), source["description"].clone());
    let properties = action["properties"]
        .as_object_mut()
        .expect("action properties");
    if configuration.actions.contains(&SequenceAction::Pose)
        || configuration.actions.contains(&SequenceAction::Joints)
    {
        let goal = properties.get_mut("goal").expect("goal schema");
        select_goal_variants(goal, configuration);
    } else {
        properties.remove("goal");
        properties.remove("waypoints");
    }
    if let Some(points) = properties.get_mut("waypoints") {
        let fields = points["items"]["properties"]
            .as_object_mut()
            .expect("waypoint fields");
        select_goal_variants(
            fields.get_mut("at").expect("waypoint configuration"),
            configuration,
        );
        if !configuration.actions.contains(&SequenceAction::Gripper) {
            fields.remove("gripper_opening");
        }
    }
    if configuration.actions.contains(&SequenceAction::Gripper) {
        let opening = properties
            .get_mut("gripper_opening")
            .expect("opening schema");
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
