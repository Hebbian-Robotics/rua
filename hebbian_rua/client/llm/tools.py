"""Parse model tool inputs with station-derived Python contract models.

Model-facing definitions are generated from the Rust catalog; these models
adapt the inputs the existing station client consumes.
"""

from pydantic import BaseModel, ConfigDict, Field

from hebbian_rua.client.contract.station_api import (
    ActionSequenceStep,
    MoveArmsCommand,
    PosePreviewCandidate,
    StationSetGripperCommand,
    StopCommand,
)
from hebbian_rua.client.llm.generated_robot_tools import ROBOT_TOOL_CATALOG
from hebbian_rua.client.llm.types import RobotToolSpec


class PreviewPoseInput(BaseModel):
    """Independent pose candidates for the selected arm."""

    model_config = ConfigDict(extra="forbid", frozen=True)
    poses: list[PosePreviewCandidate] = Field(min_length=1)


class MoveArmsInput(MoveArmsCommand):
    """Arguments for a coordinated move."""


class ArmGripperInput(StationSetGripperCommand):
    """A gripper command with the destination arm."""

    arm: str


class StopInput(StopCommand):
    """Arguments for stopping the selected arm."""


class RehomeInput(BaseModel):
    """Return the selected arm to its enable-time pose."""

    model_config = ConfigDict(extra="forbid", frozen=True)


class ObserveInput(BaseModel):
    """Get fresh camera views and the arms' states without commanding motion.

    Optionally wait up to 10 seconds before capturing, for objects to settle.
    The fresh observation is supplied after this tool returns.
    """

    model_config = ConfigDict(extra="forbid", frozen=True)

    wait_seconds: float = Field(default=0.0, ge=0.0, le=10.0, allow_inf_nan=False)


class ActionSequenceInput(BaseModel):
    """Execute steps in order. Each step can move and set the gripper together;
    a single step is one action. Each movement settles before the next step.
    The result reports completed steps and why execution stopped, if applicable."""

    model_config = ConfigDict(extra="forbid", frozen=True)
    steps: list[ActionSequenceStep] = Field(min_length=1)


type RobotToolInput = (
    PreviewPoseInput
    | MoveArmsInput
    | ArmGripperInput
    | StopInput
    | RehomeInput
    | ObserveInput
    | ActionSequenceInput
)

ROBOT_TOOL_INPUTS: dict[str, type[RobotToolInput]] = {
    "action_sequence": ActionSequenceInput,
    "preview_pose": PreviewPoseInput,
    "move_arms": MoveArmsInput,
    "set_arm_gripper": ArmGripperInput,
    "stop": StopInput,
    "rehome": RehomeInput,
    "observe": ObserveInput,
}


ROBOT_TOOLS: list[RobotToolSpec] = [
    RobotToolSpec(
        name=tool["name"],
        description=tool["description"],
        parameters_schema=tool["parameters"],
    )
    for tool in ROBOT_TOOL_CATALOG
]
