from pathlib import Path
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter

from hebbian_rua.client.contract.station_api import BasePose
from hebbian_rua.client.contract.wire_types import FiniteFloat

ArmName = Annotated[str, Field(pattern=r"^[A-Za-z0-9_-]+$")]
ARM_NAMES = TypeAdapter(list[ArmName])


class SimulationModel(BaseModel):
    effort_unit: Literal["newton_metres", "percent_of_rated_load"]
    initial_joint_positions_radians: list[FiniteFloat] | None
    gripper_force_fraction: Annotated[FiniteFloat, Field(gt=0, le=1)] | None

    def effort_per_newton_metre(self, rated_actuator_force: float) -> float:
        return (
            100 / rated_actuator_force
            if self.effort_unit == "percent_of_rated_load"
            else 1.0
        )


class StationDescription(BaseModel):
    joint_names: list[str]


class ArmConfig(BaseModel):
    # The fixture consumes the station's resolved fields alongside other controller settings.
    model_config = ConfigDict(extra="ignore")

    urdf: Path
    driven_joints: list[str]
    tool_link: str
    base_pose: BasePose | None
    lower_limits: list[FiniteFloat]
    upper_limits: list[FiniteFloat]
    joint_effort_limits: list[FiniteFloat]
    station: StationDescription
    simulation_model: SimulationModel


def load_station_arms(socket_directory: Path) -> dict[str, ArmConfig]:
    manifest = socket_directory / ".simulator-arms.json"
    if not manifest.is_file():
        raise FileNotFoundError(
            f"{manifest} is missing. Start the station with a simulator profile first."
        )
    names = ARM_NAMES.validate_json(manifest.read_bytes())
    if not names or len(set(names)) != len(names):
        raise ValueError(f"{manifest} must list distinct simulator arms")
    return {
        name: ArmConfig.model_validate_json(
            (socket_directory / f"{name}.json").read_bytes()
        )
        for name in names
    }
