use hebbian_contracts::{CornerPlan, PathLimited, PoseLimited, PoseOrientation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedArmAction {
    pub arm: String,
    pub movement: Option<PreparedMovement>,
    pub gripper: Option<PreparedGripper>,
}

impl PreparedArmAction {
    #[must_use]
    pub fn from_group_move(label: &str, part: hebbian_contracts::ArmMove) -> Self {
        Self {
            arm: part.arm,
            movement: Some(PreparedMovement::Trajectory {
                label: label.to_owned(),
                plan: part.plan,
                joint_speed: part.joint_speed,
                path_limited: part.path_limited,
                pose_limited: part.pose_limited,
            }),
            gripper: None,
        }
    }
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
