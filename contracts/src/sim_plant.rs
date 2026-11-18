//! The messages of the simulator socket. `hebbian-robot-use-server schema --sim-plant` writes
//! their JSON Schema, and the Python models in `hebbian_rua.client` are generated from it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the simulator measures after a step. Efforts are in the profile's effort
/// unit (percent of rated for the SO-101 profiles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimState {
    /// The arm's joints, in the order of the hello's `joint_names`.
    pub positions: Vec<f64>,
    pub velocities: Vec<f64>,
    pub efforts: Vec<f64>,
    /// The jaw's angle in the URDF's gripper joint; present when the arm has one.
    #[serde(default)]
    pub gripper_angle: Option<f64>,
}

/// The simulator's first line: who it is, and where the arm starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimHello {
    pub protocol_version: u32,
    /// The URDF joint names the simulator serves: the arm's, then the gripper's
    /// when it has one. They must equal the executor's driven joints.
    pub joint_names: Vec<String>,
    pub state: SimState,
}

/// What the executor sends each cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimStep {
    /// Position targets for the arm's joints.
    pub positions: Vec<f64>,
    /// The jaw's target angle, when the arm has a gripper.
    #[serde(default)]
    pub gripper_angle: Option<f64>,
    /// How much simulated time this step lasts.
    pub seconds: f64,
    /// Whether the arm has torque. When it does not, the targets are the arm's
    /// own positions and the simulator should let it rest where it is.
    pub powered: bool,
}

/// The messages of the simulator socket, as one type for the schema.
#[derive(JsonSchema)]
pub struct SimPlantContract {
    /// The simulator's first line.
    pub hello: SimHello,
    /// The simulator's reply to each step.
    pub state: SimState,
    /// The executor's line each cycle.
    pub step: SimStep,
}
