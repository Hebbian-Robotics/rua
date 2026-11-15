"""Readable model responses; station records retain their original SI values."""

import math

# Only fields with a defined unit are converted. Quaternions and approach
# vectors remain exact because clients may send them back as orientations.
_POSITION_FIELDS = {
    "position": "position_mm",
    "target_position": "target_position_mm",
    "tool_position": "tool_position_mm",
    "surface_height_meters": "surface_height_mm",
    "leeway_meters": "leeway_mm",
    "model_points": "model_points_mm",
    "position_error_meters": "position_error_mm",
    "position_shortfall_meters": "position_shortfall_mm",
}
_ANGLE_FIELDS = {
    "joints": "joints_degrees",
    "measured_joints": "measured_joints_degrees",
    "commanded_joints": "commanded_joints_degrees",
    "rest_pose": "rest_pose_degrees",
    "target_joints": "target_joints_degrees",
    "goal_joints": "goal_joints_degrees",
    "joint_error_radians": "joint_error_degrees",
    "joint_errors_radians": "joint_errors_degrees",
    "orientation_error_radians": "orientation_error_degrees",
    "orientation_shortfall_radians": "orientation_shortfall_degrees",
    "joint_speed": "joint_speed_degrees_per_second",
    "max_joint_speed": "max_joint_speed_degrees_per_second",
}


def _round_half_away_from_zero(value: float, decimals: int) -> float:
    scale = 10**decimals
    return math.copysign(math.floor(abs(value) * scale + 0.5) / scale, value)


def _scale_numbers(value: object, factor: float) -> object:
    if isinstance(value, list):
        return [_scale_numbers(item, factor) for item in value]
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return _round_half_away_from_zero(value * factor, 1)
    return value


def model_response(value: object, field_name: str = "") -> object:
    """Convert model-facing JSON without altering command or telemetry records."""
    if field_name in {"quaternion", "target_quaternion", "approach", "jaw_yaw_degrees"}:
        return value
    if isinstance(value, dict):
        formatted = {}
        for key, item in value.items():
            if item is None or (key == "checkpoints" and item == []):
                continue
            if key in _POSITION_FIELDS:
                formatted[_POSITION_FIELDS[key]] = _scale_numbers(item, 1000.0)
            elif key in _ANGLE_FIELDS:
                formatted[_ANGLE_FIELDS[key]] = _scale_numbers(item, 180.0 / math.pi)
            else:
                formatted[key] = model_response(item, key)
        return formatted
    if isinstance(value, list):
        return [model_response(item, field_name) for item in value]
    if isinstance(value, float):
        return _round_half_away_from_zero(
            value,
            3
            if "unix_seconds" in field_name
            or field_name in {"accepted_at", "sampled_at", "finished_at"}
            else 4,
        )
    return value
