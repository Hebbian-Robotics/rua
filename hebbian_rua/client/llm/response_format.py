"""Readable model responses; station records retain their original SI values."""

import json
import math
from collections.abc import Collection
from dataclasses import replace
from typing import Literal

from hebbian_rua.client.llm.generated_robot_tools import (
    MODEL_OUTPUT_UNITS,
    MODEL_RESPONSE_FIELDS,
)
from hebbian_rua.client.llm.types import (
    ModelContent,
    RobotOutput,
    RobotToolCall,
    RobotToolResult,
    SequenceImageHistory,
)

_OUTPUT_CONVERSIONS = {
    field: (name, scale) for field, name, scale in MODEL_OUTPUT_UNITS
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


def _model_command_record(record: dict) -> dict:
    command, phase = record.get("command"), record.get("phase")
    if (
        not isinstance(command, dict)
        or not isinstance(phase, dict)
        or not isinstance(record.get("command_id"), str)
        or not isinstance(command.get("kind"), str)
        or phase.get("kind")
        not in ("pending", "not_started", "finished", "outcome_unknown")
    ):
        return record
    result = {
        key: item for key, item in record.items() if key not in {"command", "phase"}
    }
    result.update(
        (key, item) for key, item in command.items() if key not in {"kind", "goal_pose"}
    )
    pose = command.get("goal_pose")
    if isinstance(pose, dict):
        result["goal_position"] = pose.get("position")
        result["goal_quaternion"] = pose.get("quaternion")
    phase_kind = phase["kind"]
    if phase_kind == "finished":
        completion = phase.get("completion")
        result["completion"] = completion
        result["achieved"] = phase.get("achieved")
        if command["kind"] == "move" and isinstance(completion, dict):
            outcome = completion.get("outcome")
            match outcome:
                case "completed":
                    result["goal_status"] = (
                        "limited"
                        if command.get("pose_limited") is not None
                        or command.get("path_limited") is not None
                        else "reached"
                    )
                case "stopped_short":
                    result["goal_status"] = "stopped_short"
                case _:
                    result["goal_status"] = "partial"
    elif phase_kind in {"not_started", "outcome_unknown"}:
        result["completion"] = {
            "outcome": "interrupted"
            if phase_kind == "outcome_unknown"
            else phase.get("outcome"),
            "detail": phase.get("detail"),
            "finished_at": phase.get("finished_at"),
        }
        if phase_kind == "not_started":
            key = "goal_status" if command["kind"] == "move" else "phase"
            result[key] = "not_started"
    return result


def model_response(value: object) -> object:
    """Convert an SI payload to readable model units without summarizing its history."""
    return _format_model_field(value, "")


def _format_model_field(value: object, field_name: str) -> object:
    if field_name in {"approach", "jaw_yaw_degrees"}:
        return value
    if isinstance(value, dict):
        value = _model_command_record(value)
        formatted = {}
        for key, item in value.items():
            if item is None or (key == "checkpoints" and item == []):
                continue
            if conversion := _OUTPUT_CONVERSIONS.get(key):
                model_field, factor = conversion
                formatted[model_field] = _scale_numbers(item, factor)
            else:
                formatted[key] = _format_model_field(item, key)
        return formatted
    if isinstance(value, list):
        return [_format_model_field(item, field_name) for item in value]
    if isinstance(value, float):
        # Rounded quaternion components are normalized by station parsing on reuse.
        return _round_half_away_from_zero(
            value,
            3
            if "unix_seconds" in field_name
            or field_name in {"accepted_at", "sampled_at", "finished_at"}
            else 4,
        )
    return value


def _compact_earlier_step(value: object) -> object:
    if not isinstance(value, list):
        return concise_output(value)
    entries: list[object] = []
    for item in value:
        if not isinstance(item, dict) or not isinstance(item.get("record"), dict):
            entries.append(concise_output(item))
            continue
        record = _model_command_record(item["record"])
        entries.append(
            concise_output(
                {
                    **{key: item[key] for key in ("arm",) if key in item},
                    "record": {
                        key: record[key]
                        for key in MODEL_RESPONSE_FIELDS["earlier_step"]
                        if key in record
                    },
                }
            )
        )
    return entries


def concise_output(value: object, *, keep_station_context: bool = False) -> object:
    """Keep outcomes and actionable state; omit history and rendering geometry.

    `keep_station_context` also keeps a station state's `description` (frames and
    conventions) and `lease` holder, for a caller that has no other way to learn them.
    Observations leave them out because the model is given the conventions separately.
    """
    if isinstance(value, list):
        return [
            concise_output(item, keep_station_context=keep_station_context)
            for item in value
        ]
    if not isinstance(value, dict):
        return value
    value = _model_command_record(value)
    if "arms" in value and "joint_names" in value:
        kept_state_fields = MODEL_RESPONSE_FIELDS["state"] + (
            MODEL_RESPONSE_FIELDS["station_context"] if keep_station_context else []
        )
        return {
            **{key: value[key] for key in kept_state_fields if key in value},
            "arms": [
                concise_output(
                    {
                        key: item
                        for key, item in arm.items()
                        if key in MODEL_RESPONSE_FIELDS["arm"]
                    }
                )
                for arm in value["arms"]
            ],
        }
    if "requested_steps" in value and "steps" in value:
        steps = value["steps"]
        return {
            **{key: value[key] for key in ("arms", "phase") if key in value},
            "requested_step_count": len(value["requested_steps"]),
            "recorded_step_count": len(steps),
            "steps": [
                concise_output(step)
                if index == len(steps) - 1
                else _compact_earlier_step(step)
                for index, step in enumerate(steps)
            ],
        }
    return {
        key: concise_output(item, keep_station_context=keep_station_context)
        for key, item in value.items()
        if key not in MODEL_RESPONSE_FIELDS["diagnostic"]
    }


def concise_response(value: object, *, keep_station_context: bool = False) -> object:
    """The model view: the concise layer, then readable units and rounding."""
    return model_response(
        concise_output(value, keep_station_context=keep_station_context)
    )


def summarize_output(output: RobotOutput) -> RobotOutput:
    """Select concise payload fields, preserving SI units and observation context."""
    return replace(output, data=concise_output(output.data))


def select_output_cameras(output: RobotOutput, names: Collection[str]) -> RobotOutput:
    """Select model images and their retrieval intervals without changing capture.

    Availability notices remain visible even when a camera's image is omitted.
    """
    return replace(
        output,
        cameras={name: jpeg for name, jpeg in output.cameras.items() if name in names},
        sequence_images=replace(
            output.sequence_images,
            images=[
                image
                for image in output.sequence_images.images
                if image.camera in names
            ],
        )
        if output.sequence_images is not None
        else None,
        camera_intervals={
            name: interval
            for name, interval in output.camera_intervals.items()
            if name in names
        },
    )


def render_output(
    output: RobotOutput, *, units: Literal["model", "si"] = "model"
) -> ModelContent:
    """Render once, converting to readable model units or retaining exact SI data."""
    data = model_response(output.data) if units == "model" else output.data
    text = (
        data
        if isinstance(data, str)
        else json.dumps(data, separators=(",", ":") if units == "model" else None)
    )
    cameras = {}
    if output.sequence_images is not None:
        history = output.sequence_images
        for index, image in enumerate(history.images):
            label = (
                f"Sequence {history.call_id} image {index + 1}: {image.camera}, "
                f"+{image.elapsed_seconds:.3f}s, {image.boundary}; "
                f"retrieved {image.retrieved_between_unix_seconds} Unix seconds"
            )
            cameras[label] = image.jpeg
    cameras.update(output.cameras)
    if output.kind == "result":
        return ModelContent(text, cameras)
    lines = [
        *output.notes,
        f"Selected arm: {output.selected_arm}.",
        "Arm state: " + text,
    ]
    if output.missing_optional_cameras:
        lines.append(
            "Optional cameras unavailable: "
            + ", ".join(output.missing_optional_cameras)
        )
    if output.camera_intervals:
        intervals = (
            {
                name: tuple(round(timestamp, 3) for timestamp in interval)
                for name, interval in output.camera_intervals.items()
            }
            if units == "model"
            else output.camera_intervals
        )
        lines.append(
            "Camera retrieval intervals (Unix seconds): " + json.dumps(intervals)
        )
    return ModelContent("\n".join(lines), cameras)


def default_formatter(output: RobotOutput) -> ModelContent:
    """A concise model view; full station data remains on the result/observation."""
    return render_output(summarize_output(output))


def full_formatter(output: RobotOutput) -> ModelContent:
    """Show the complete payload in original SI units, without rounding."""
    return render_output(output, units="si")


def result_from_data(
    call: RobotToolCall,
    data: object,
    *,
    is_error: bool,
    halt: bool = False,
    interrupted: bool = False,
    cameras: dict[str, bytes] | None = None,
    sequence_images: SequenceImageHistory | None = None,
    observed_revisions: dict[str, int] | None = None,
) -> RobotToolResult:
    """Retain a full result alongside its default model view and control flags."""
    content = default_formatter(
        RobotOutput(
            "result",
            data,
            call=call,
            cameras=cameras or {},
            sequence_images=sequence_images,
        )
    )
    return RobotToolResult(
        call,
        content.text,
        is_error=is_error,
        halt=halt,
        interrupted=interrupted,
        data=data,
        cameras=content.cameras,
        sequence_images=sequence_images,
        observed_revisions=observed_revisions,
    )
