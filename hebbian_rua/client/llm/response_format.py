"""Readable model responses; station records retain their original SI values."""

import json
from collections.abc import Collection
from dataclasses import replace
from typing import Literal

from hebbian_rua.bindings import _native
from hebbian_rua.client.llm.types import (
    ModelContent,
    RobotOutput,
    RobotToolCall,
    RobotToolResult,
    SequenceImageHistory,
)

concise_response = _native.concise_response
concise_output = _native.concise_output
model_response = _native.model_response


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
