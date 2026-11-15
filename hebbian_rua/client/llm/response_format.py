"""Readable model responses; station records retain their original SI values."""

import json
import math

from hebbian_rua.client.llm.types import (
    ModelContent,
    RobotOutput,
    RobotToolCall,
    RobotToolResult,
)

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
    "measured_joint_velocities": "measured_joint_velocities_degrees_per_second",
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


def _compact_output(value: object) -> object:
    """Keep outcomes and actionable state; omit history and rendering geometry."""
    if isinstance(value, list):
        return [_compact_output(item) for item in value]
    if not isinstance(value, dict):
        return value
    if "arms" in value and "joint_names" in value:
        arm_fields = {
            "arm",
            "mode",
            "status",
            "fault",
            "interlocks",
            "recovery_issue",
            "revision",
            "sampled_at_unix_seconds",
            "measured_joints",
            "measured_joint_velocities",
            "measured_joint_efforts",
            "tool",
            "gripper_opening",
        }
        return {
            **{
                key: value[key]
                for key in ("robot", "joint_names", "max_joint_speed", "surface_guard")
                if key in value
            },
            "arms": [
                _compact_output(
                    {key: item for key, item in arm.items() if key in arm_fields}
                )
                for arm in value["arms"]
            ],
        }
    if "requested_steps" in value and "steps" in value:
        steps = value["steps"]
        return {
            **{key: value[key] for key in ("id", "arm", "phase") if key in value},
            "requested_step_count": len(value["requested_steps"]),
            "recorded_step_count": len(steps),
            "last_step": _compact_output(steps[-1]) if steps else [],
        }
    omitted = {
        "checkpoints",
        "trace_context",
        "model_points",
        "accepted_at",
        "finished_at",
    }
    return {
        key: _compact_output(item) for key, item in value.items() if key not in omitted
    }


def default_formatter(output: RobotOutput) -> ModelContent:
    """A concise model view; full station data remains on the result/observation."""
    formatted = model_response(_compact_output(output.data))
    if output.kind == "result":
        return ModelContent(
            output.data
            if isinstance(output.data, str)
            else json.dumps(formatted, separators=(",", ":"))
        )
    lines = [
        *output.notes,
        f"Selected arm: {output.selected_arm}.",
        "Arm state: " + json.dumps(formatted, separators=(",", ":")),
    ]
    if output.missing_optional_cameras:
        lines.append(
            "Optional cameras unavailable: "
            + ", ".join(output.missing_optional_cameras)
        )
    if output.camera_intervals:
        lines.append(
            "Camera retrieval intervals (Unix seconds): "
            + json.dumps(
                {
                    name: [round(timestamp, 3) for timestamp in interval]
                    for name, interval in output.camera_intervals.items()
                }
            )
        )
    return ModelContent("\n".join(lines), dict(output.cameras))


def full_formatter(output: RobotOutput) -> ModelContent:
    """Show the complete payload in original SI units, without rounding."""
    text = output.data if isinstance(output.data, str) else json.dumps(output.data)
    if output.kind == "observation":
        text = "\n".join(
            [
                *output.notes,
                f"Selected arm: {output.selected_arm}.",
                text,
                "Optional cameras unavailable: "
                + ", ".join(output.missing_optional_cameras),
                "Camera retrieval intervals (Unix seconds): "
                + json.dumps(output.camera_intervals),
            ]
        )
    return ModelContent(text, dict(output.cameras))


def result_from_data(
    call: RobotToolCall,
    data: object,
    *,
    is_error: bool,
    halt: bool = False,
    interrupted: bool = False,
) -> RobotToolResult:
    """Retain a full result alongside its default model view and control flags."""
    return RobotToolResult(
        call,
        default_formatter(RobotOutput("result", data, call=call)).text,
        is_error=is_error,
        halt=halt,
        interrupted=interrupted,
        raw_data=data,
    )
