use hebbian_contracts::{
    ActionSequenceRequest, MoveArmsCommand, ObservationSource, PosePreviewCandidateResult,
    PreviewPose, StationState,
};

pub(crate) enum ToolOperation {
    ActionSequence {
        action_sequence: ActionSequenceRequest,
    },
    PosePreviews {
        poses: Vec<PreviewPose>,
    },
    MoveArms {
        command: MoveArmsCommand,
    },
    SetGripper {
        arm: String,
        opening: hebbian_contracts::GripperOpening,
        max_joint_speed: Option<hebbian_contracts::JointSpeed>,
    },
    Stop {
        arm: String,
    },
    Rehome {
        arm: String,
    },
    Observe {
        source: ObservationSource,
        wait_seconds: f64,
    },
}

pub(crate) enum ToolOperationAnswer {
    Refused {
        reason: hebbian_contracts::RejectionReason,
        detail: String,
    },
    PosePreviews {
        results: Vec<PosePreviewCandidateResult>,
    },
    ActionSequence {
        execution: crate::SequenceExecution,
    },
    Observation {
        state: Box<StationState>,
    },
}
