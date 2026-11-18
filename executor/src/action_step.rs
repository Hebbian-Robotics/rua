use hebbian_contracts::{CornerPlan, PathLimited, PoseLimited, PoseOrientation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedArmAction {
    pub arm: String,
    pub movement: Option<PreparedMovement>,
    pub gripper: Option<PreparedGripper>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PreparedMovement {
    Trajectory {
        label: String,
        plan: CornerPlan,
        joint_speed: f64,
        path_limited: Option<PathLimited>,
        pose_limited: Option<PoseLimited>,
    },
    PolicyPose {
        label: String,
        position: [f64; 3],
        orientation: PoseOrientation,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedGripper {
    pub angle: f64,
    pub joint_speed: Option<f64>,
}
