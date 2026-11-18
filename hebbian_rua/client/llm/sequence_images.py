"""Bounded client-side visual history, independent of station recording."""

import asyncio
from collections.abc import AsyncGenerator, Callable, Sequence
from contextlib import asynccontextmanager
from dataclasses import replace
from time import monotonic
from typing import Literal

import httpx2

from hebbian_rua.client.llm.types import SequenceImage, SequenceImageHistory
from hebbian_rua.client.station_client import StationClient
from hebbian_rua.client.station_types import CameraUnavailable

MAXIMUM_BYTES = 16 * 1024 * 1024
MAXIMUM_IMAGES = 64
MAXIMUM_MODEL_BYTES = 2 * 1024 * 1024
MAXIMUM_MODEL_IMAGES = 8
RETENTION_SECONDS = 300


class SequenceImages:
    """Share latest-sequence evidence across tool views under hard retention limits."""

    def __init__(self) -> None:
        self._history: SequenceImageHistory | None = None
        self._bytes = 0
        self._expires_at = 0.0
        self._expiry: asyncio.TimerHandle | None = None

    def begin(self, call_id: str) -> SequenceImageHistory:
        self.clear()
        self._history = SequenceImageHistory(call_id)
        self._expires_at = float("inf")
        return self._history

    def _refresh_expiry(self) -> None:
        self._expires_at = monotonic() + RETENTION_SECONDS
        if self._expiry is not None:
            self._expiry.cancel()
        self._expiry = asyncio.get_running_loop().call_later(
            RETENTION_SECONDS, self.clear
        )

    def add(self, history: SequenceImageHistory, image: SequenceImage) -> None:
        if history is not self._history:
            return
        if len(image.jpeg) > MAXIMUM_MODEL_BYTES:
            history.missed_samples += 1
            return
        history.images.append(image)
        self._bytes += len(image.jpeg)
        while len(history.images) > MAXIMUM_IMAGES or self._bytes > MAXIMUM_BYTES:
            self._bytes -= len(history.images.pop(1).jpeg)
            history.truncated = True

    def finish(self, history: SequenceImageHistory) -> None:
        if history is self._history:
            self._refresh_expiry()

    def snapshot(self) -> SequenceImageHistory | None:
        if monotonic() >= self._expires_at:
            self.clear()
        if self._history is None:
            return None
        images = self._history.images
        count = min(MAXIMUM_MODEL_IMAGES, len(images))
        selected: list[SequenceImage] = []
        size = 0
        for index in reversed(range(count)):
            image = images[round(index * (len(images) - 1) / max(1, count - 1))]
            if size + len(image.jpeg) <= MAXIMUM_MODEL_BYTES:
                selected.append(image)
                size += len(image.jpeg)
        return replace(self._history, images=list(reversed(selected)))

    def clear(self) -> None:
        if self._history is not None:
            self._history.images.clear()
        self._history = None
        self._bytes = 0
        if self._expiry is not None:
            self._expiry.cancel()
            self._expiry = None


@asynccontextmanager
async def capture_sequence_images(
    history_store: SequenceImages,
    call_id: str,
    cameras: Sequence[str],
    station: StationClient,
) -> AsyncGenerator[Callable[[str], None]]:
    """Sample off the command path; slow or failed snapshots never stop motion."""
    if not cameras:
        history_store.clear()
        yield lambda _sequence_id: None
        return
    history = history_store.begin(call_id)
    started_at = monotonic()
    capture_task: asyncio.Task[None] | None = None
    watcher: asyncio.Task[None] | None = None
    camera_offset = 0

    async def capture(boundary: Literal["start", "motion", "step", "end"]) -> None:
        nonlocal camera_offset
        ordered_cameras = [*cameras[camera_offset:], *cameras[:camera_offset]]
        camera_offset = (camera_offset + 1) % len(cameras)
        # CONSTRAINT: One bounded batch at a time, including all configured cameras.
        try:
            async with asyncio.timeout(0.5):
                for camera in ordered_cameras:
                    try:
                        snapshot = await station.camera_jpeg(
                            camera, maximum_bytes=MAXIMUM_MODEL_BYTES
                        )
                        history_store.add(
                            history,
                            SequenceImage(
                                camera,
                                snapshot.jpeg,
                                snapshot.retrieved_between_unix_seconds,
                                monotonic() - started_at,
                                boundary,
                            ),
                        )
                    except (
                        CameraUnavailable,
                        httpx2.HTTPError,
                        OSError,
                        ValueError,
                        RuntimeError,
                    ):
                        history.missed_samples += 1
        except TimeoutError:
            history.missed_samples += 1

    def sample(boundary: Literal["start", "motion", "step", "end"]) -> None:
        nonlocal capture_task
        if capture_task is None or capture_task.done():
            capture_task = asyncio.create_task(capture(boundary))

    async def periodic() -> None:
        while True:
            await asyncio.sleep(0.25)
            sample("motion")

    async def watch_steps(sequence_id: str) -> None:
        await _sample_step_boundaries(station, sequence_id, lambda: sample("step"))

    def set_sequence_id(sequence_id: str) -> None:
        nonlocal watcher
        watcher = asyncio.create_task(watch_steps(sequence_id))

    sample("start")
    timer = asyncio.create_task(periodic())
    try:
        yield set_sequence_id
    finally:
        try:
            tasks = [
                timer,
                *([watcher] if watcher else []),
                *([capture_task] if capture_task else []),
            ]
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            await capture("end")
        finally:
            history_store.finish(history)


async def _sample_step_boundaries(
    station: StationClient, sequence_id: str, sample: Callable[[], None]
) -> None:
    last_step = -1
    try:
        async for sequence in station.action_sequence_updates():
            if sequence is None or sequence.id != sequence_id:
                continue
            if sequence.phase.state != "running":
                return
            if sequence.phase.step != last_step:
                last_step = sequence.phase.step
                sample()
    except (httpx2.HTTPError, OSError):
        return
