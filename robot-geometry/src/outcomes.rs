//! Geometry solver requests and outcomes, shared by local and serialized callers.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Why a solve stopped short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IkFailure {
    /// The best pose it found has a joint at its limit, and no step improves it.
    AtJointLimit,
    /// No step improves the best pose it found, away from any limit: the target
    /// is out of reach, or the arm is in a singular pose that cannot move toward it.
    Stalled,
    /// It was still improving when it ran out of iterations.
    IterationLimit,
    /// The target is farther from the arm's first joint than the arm's links
    /// reach; the best pose stretches toward it.
    OutOfReach,
}

/// What orientation the tool must have.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum OrientationGoal {
    /// The tool reaches this orientation, `[x, y, z, w]`, as well as the position.
    #[schemars(title = "RequiredOrientation")]
    Required { quaternion: [f64; 4] },
    /// Only the position matters. The solve prefers the orientation the tool has
    /// at the seed, weighed far below position, and reports where it landed.
    #[schemars(title = "FreeOrientation")]
    Free,
}

/// Where a solve left the arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IkPose {
    pub joints: Vec<f64>,
    pub tool_position: [f64; 3],
    pub tool_quaternion: [f64; 4],
    pub position_error_meters: f64,
    /// From the required orientation, or from the seed's when it was free.
    pub orientation_error_radians: f64,
}

/// A solve's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IkResult {
    /// The tool is within tolerance of the target.
    #[schemars(title = "IkSolved")]
    Solved(IkPose),
    /// It is not; `best` is the pose with the smallest position error found,
    /// always inside the joint limits.
    #[schemars(title = "IkFailed")]
    Failed { reason: IkFailure, best: IkPose },
}
