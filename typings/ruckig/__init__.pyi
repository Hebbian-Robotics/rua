from enum import Enum

class Result(Enum):
    Working = 0
    Finished = 1

class Synchronization(Enum):
    Time = 0
    TimeIfNecessary = 1
    Phase = 2
    No = 3

class RuckigError(Exception): ...

class InputParameter:
    def __init__(self, degrees_of_freedom: int) -> None: ...
    current_position: list[float]
    current_velocity: list[float]
    current_acceleration: list[float]
    target_position: list[float]
    target_velocity: list[float]
    target_acceleration: list[float]
    max_velocity: list[float]
    max_acceleration: list[float]
    max_jerk: list[float]
    synchronization: Synchronization
    minimum_duration: float | None

class Trajectory:
    def __init__(self, degrees_of_freedom: int) -> None: ...
    duration: float
    def at_time(self, time: float) -> tuple[list[float], list[float], list[float]]: ...

class Ruckig:
    def __init__(self, degrees_of_freedom: int) -> None: ...
    def calculate(
        self, parameters: InputParameter, trajectory: Trajectory
    ) -> Result: ...
