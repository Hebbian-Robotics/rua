"""Explicit motion units for constructing native station values."""

from collections.abc import Sequence
from typing import Literal

from hebbian_rua.bindings._native import METRES_PER_MILLIMETRE, RADIANS_PER_DEGREE

LengthUnit = Literal["m", "mm"]
AngleUnit = Literal["rad", "deg"]


def position_metres(position: Sequence[float], *, unit: LengthUnit) -> list[float]:
    """Convert an XYZ position to metres for station motion commands."""
    if len(position) != 3:
        raise ValueError("A position requires three coordinates")
    match unit:
        case "m":
            return list(position)
        case "mm":
            return [coordinate * METRES_PER_MILLIMETRE for coordinate in position]
        case _:
            raise ValueError(f"Invalid length unit: {unit!r}")


def angular_radians(value: float, *, unit: AngleUnit) -> float:
    """Convert an angle or angular rate to radians, retaining its time denominator."""
    match unit:
        case "rad":
            return value
        case "deg":
            return value * RADIANS_PER_DEGREE
        case _:
            raise ValueError(f"Invalid angle unit: {unit!r}")


def angular_degrees(value: float, *, unit: AngleUnit) -> float:
    """Convert an angle or angular rate to degrees, retaining its time denominator."""
    match unit:
        case "rad":
            return value * (1 / RADIANS_PER_DEGREE)
        case "deg":
            return value
        case _:
            raise ValueError(f"Invalid angle unit: {unit!r}")


def joint_radians(joints: Sequence[float], *, unit: AngleUnit) -> list[float]:
    """Convert a joint target to radians in the supplied joint order."""
    return [angular_radians(angle, unit=unit) for angle in joints]
