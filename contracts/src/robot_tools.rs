//! Model-facing tool definitions derived from the station contract.
//!
//! The model supplies orientation fields without the station's `kind` tag;
//! SDK adapters add that tag before sending the request.

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ActionSequenceStep, MoveArmsCommand, PosePreviewCandidate, StationCommand};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewPoseArguments {
    #[schemars(length(min = 1))]
    pub poses: Vec<PosePreviewCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceArguments {
    #[schemars(length(min = 1))]
    pub steps: Vec<ActionSequenceStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveArguments {
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 10.0))]
    pub wait_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopArguments {}

/// Station operation, or local observation behavior, used by a model tool.
#[derive(Debug, Clone, Copy, Serialize)]
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

/// A provider-independent model tool specification.
#[derive(Debug, Clone, Serialize)]
pub struct RobotToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
    pub operation: RobotToolOperation,
}

fn model_arguments<T: JsonSchema>() -> Value {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("schema is serializable");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    adapt(&mut schema);
    schema
}

// Existing SDK adapters infer these tags from the argument shape. Movement
// tags are retained in the schema but optional, matching their defaulting.
fn adapt(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("format");
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
                kind.insert("default".into(), json!(tag));
                if let Some(Value::Array(required)) = object.get_mut("required") {
                    required.retain(|field| field != "kind");
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
        _ => {}
    }
}

// The gripper tool adds an arm selector to the station's command fields.
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
pub fn robot_tool_catalog() -> Vec<RobotToolDefinition> {
    vec![
        RobotToolDefinition {
            name: "action_sequence",
            operation: RobotToolOperation::StartActionSequence,
            description: "Execute steps in order; each step can move and set the gripper together. The result reports completed steps and why execution stopped.".into(),
            parameters: model_arguments::<ActionSequenceArguments>(),
        },
        RobotToolDefinition {
            name: "preview_pose",
            operation: RobotToolOperation::Preview,
            description: "Preview independent world-frame pose candidates without moving. Supply each candidate with a unique UUID v4 id, an optional label, and a pose. Results retain their ids and input order; each includes a projection or refusal. Omitting orientation lets the solver choose one.".into(),
            parameters: model_arguments::<PreviewPoseArguments>(),
        },
        RobotToolDefinition {
            name: "move_arms",
            operation: RobotToolOperation::MoveArms,
            description: "Move named arms together. Pose targets use a world position and orientation; joint targets use radians. Model guards do not detect collisions between arms or with workspace objects.".into(),
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
            description: "Return this arm to the joint pose recorded when it was enabled. This moves the arm without changing the gripper; ensure the path and workspace are clear.".into(),
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
            description: "Return the station state after an optional wait without commanding motion. Camera images can be obtained separately.".into(),
            parameters: model_arguments::<ObserveArguments>(),
        },
    ]
}

/// Robot conventions and limitations independent of a particular station.
#[must_use]
pub fn robot_tool_context() -> Value {
    json!({
        "frames": "Positions are in the station world frame. Tool inputs use meters, joint angles use radians, and joint speeds use radians per second.",
        "orientation": format!("The approach is down (world -Z), forward (world +X), or a world direction vector. Jaw yaw is in degrees about minus approach; zero is projected world +X, or world +Y when the absolute normalized approach X exceeds {}. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.", crate::JAW_REFERENCE_SWITCH_APPROACH_X),
        "gripper": "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. A step speed overrides its movement speed for both movement and gripper.",
        "outcomes": "A completed move may still have pose_limited or path_limited: the planned motion finished but did not reach the requested target. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are clamped.",
        "limitations": "Model guards can check self-collision and a horizontal surface, but do not detect workspace objects or other arms and do not establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified."
    })
}
