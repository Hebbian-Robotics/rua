use hebbian_contracts::{CornerPlan, JointSpeed, PathLimited, PoseLimited, PoseOrientation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedArmAction {
    arm: String,
    #[serde(flatten)]
    action: PreparedAction,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PreparedAction {
    Move {
        movement: PreparedMovement,
    },
    Gripper {
        gripper: PreparedGripper,
    },
    MoveAndGripper {
        movement: PreparedMovement,
        gripper: PreparedGripper,
    },
}

pub(crate) enum SingleCommandRoute {
    Prepared(PreparedArmAction),
    Direct(hebbian_contracts::Command),
}

impl PreparedArmAction {
    pub(crate) fn movement(arm: String, movement: PreparedMovement) -> Self {
        Self {
            arm,
            action: PreparedAction::Move { movement },
        }
    }

    pub(crate) fn gripper(arm: String, gripper: PreparedGripper) -> Self {
        Self {
            arm,
            action: PreparedAction::Gripper { gripper },
        }
    }

    pub(crate) fn new(
        arm: String,
        movement: Option<PreparedMovement>,
        gripper: Option<PreparedGripper>,
    ) -> Option<Self> {
        match (movement, gripper) {
            (Some(movement), Some(gripper)) => Some(Self {
                arm,
                action: PreparedAction::MoveAndGripper { movement, gripper },
            }),
            (Some(movement), None) => Some(Self::movement(arm, movement)),
            (None, Some(gripper)) => Some(Self::gripper(arm, gripper)),
            (None, None) => None,
        }
    }

    pub(crate) fn from_single_command(
        arm: String,
        command: hebbian_contracts::Command,
    ) -> SingleCommandRoute {
        use hebbian_contracts::Command;

        match command {
            Command::Move {
                label,
                plan,
                joint_speed,
                path_limited,
                pose_limited,
            } => SingleCommandRoute::Prepared(Self::movement(
                arm,
                PreparedMovement::Trajectory {
                    label,
                    plan,
                    joint_speed,
                    path_limited,
                    pose_limited,
                },
            )),
            Command::ReachPose {
                label,
                position,
                orientation,
            } => SingleCommandRoute::Prepared(Self::movement(
                arm,
                PreparedMovement::PolicyPose {
                    label,
                    position,
                    orientation,
                },
            )),
            Command::SetGripper { angle, joint_speed } => SingleCommandRoute::Prepared(
                Self::gripper(arm, PreparedGripper { angle, joint_speed }),
            ),
            other @ (Command::Enable
            | Command::ApproveRecovery
            | Command::Disable
            | Command::Stop
            | Command::ClearFault
            | Command::Shutdown
            | Command::StreamJoints { .. }
            | Command::StreamPose { .. }) => SingleCommandRoute::Direct(other),
        }
    }

    pub(crate) fn arm(&self) -> &str {
        &self.arm
    }

    pub(crate) fn record_count(&self) -> usize {
        match self.action {
            PreparedAction::Move { .. } | PreparedAction::Gripper { .. } => 1,
            PreparedAction::MoveAndGripper { .. } => 2,
        }
    }

    pub(crate) fn uses_trajectory(&self) -> bool {
        match &self.action {
            PreparedAction::Move { movement } | PreparedAction::MoveAndGripper { movement, .. } => {
                matches!(movement, PreparedMovement::Trajectory { .. })
            }
            PreparedAction::Gripper { .. } => false,
        }
    }

    pub(crate) fn into_parts(self) -> (String, Option<PreparedMovement>, Option<PreparedGripper>) {
        let (movement, gripper) = match self.action {
            PreparedAction::Move { movement } => (Some(movement), None),
            PreparedAction::Gripper { gripper } => (None, Some(gripper)),
            PreparedAction::MoveAndGripper { movement, gripper } => (Some(movement), Some(gripper)),
        };
        (self.arm, movement, gripper)
    }

    #[must_use]
    pub fn from_group_move(label: &str, part: hebbian_contracts::ArmMove) -> Self {
        Self::movement(
            part.arm,
            PreparedMovement::Trajectory {
                label: label.to_owned(),
                plan: part.plan,
                joint_speed: part.joint_speed,
                path_limited: part.path_limited,
                pose_limited: part.pose_limited,
            },
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PreparedMovement {
    Trajectory {
        label: String,
        plan: CornerPlan,
        joint_speed: JointSpeed,
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
    pub joint_speed: Option<JointSpeed>,
}
