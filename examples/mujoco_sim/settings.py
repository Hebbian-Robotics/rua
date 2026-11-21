from pathlib import Path
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field

from hebbian_rua.client.contract.wire_types import FiniteFloat

PositiveFloat = Annotated[FiniteFloat, Field(gt=0)]


class MujocoSettings(BaseModel):
    model_config = ConfigDict(extra="forbid")
    # Floor, lights and sky for the arms to stand in.
    environment: Path = Path(__file__).with_name("environment.xml")
    # Joint angles an arm starts at, unpowered, when its profile has no rest reference.
    start_pose_radians: tuple[FiniteFloat, ...] = (0.0, -1.0, 1.0, 0.5, 0.0)
    # The arm model's actuator forcerange, the 100 % of the effort signal.
    rated_actuator_force: PositiveFloat = 2.94
    # Set explicitly when the simulation needs a particular physics integrator.
    timestep_seconds: PositiveFloat = 0.005
    integrator: Literal["euler", "implicitfast"] = "implicitfast"
    # The arm model's gripper joint and actuator, used when the profile has a gripper.
    gripper_joint: str = "gripper"
