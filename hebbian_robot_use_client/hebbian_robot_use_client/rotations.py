"""Rotation helpers that need nothing beyond the standard library."""

import math


def quaternion_from_roll_pitch_yaw(
    roll: float, pitch: float, yaw: float
) -> list[float]:
    """[x, y, z, w] of Rz(yaw) Ry(pitch) Rx(roll), angles in radians."""
    cos_roll, sin_roll = math.cos(roll / 2), math.sin(roll / 2)
    cos_pitch, sin_pitch = math.cos(pitch / 2), math.sin(pitch / 2)
    cos_yaw, sin_yaw = math.cos(yaw / 2), math.sin(yaw / 2)
    return [
        sin_roll * cos_pitch * cos_yaw - cos_roll * sin_pitch * sin_yaw,
        cos_roll * sin_pitch * cos_yaw + sin_roll * cos_pitch * sin_yaw,
        cos_roll * cos_pitch * sin_yaw - sin_roll * sin_pitch * cos_yaw,
        cos_roll * cos_pitch * cos_yaw + sin_roll * sin_pitch * sin_yaw,
    ]
