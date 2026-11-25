"""Interlock recovery and revision-fenced observation for robot-use agents.

An agent holds the lease, observes, and sends commands carrying the arm's
`revision` from that observation, so a command planned before an interruption
is refused as `stale_revision` instead of acting on an out-of-date view. See
docs/control-model.md.
"""

import asyncio
import logging

from hebbian_rua.bindings._native import CommandRefused as NativeCommandRefused
from hebbian_rua.bindings._native import HttpError
from hebbian_rua.client.contract.station_api import ArmMode, ArmState
from hebbian_rua.client.llm.robot_tools import (
    ArmCommunicationInterrupted,
    CapturedObservation,
    RequiredCameraUnavailable,
    RobotTools,
)
from hebbian_rua.client.station_client import StationClient
from hebbian_rua.client.station_types import LeaseLost

log = logging.getLogger(__name__)
INITIAL_CAMERA_RETRY_SECONDS = 0.1
MAX_CAMERA_RETRY_SECONDS = 1.0
ARM_INTERLOCKS = frozenset({"arm_disconnected", "recovery_approval_required"})


class ArmRecoveryEnded(Exception):
    """The arm was disabled or faulted before it recovered."""


async def _wait_for_change_or_lease_loss(
    client: StationClient, observed: tuple[int, int]
) -> None:
    change = asyncio.create_task(client.wait_for_change(observed))
    lost = asyncio.create_task(client.lease_lost.wait())
    try:
        await asyncio.wait({change, lost}, return_when=asyncio.FIRST_COMPLETED)
    finally:
        for task in (change, lost):
            task.cancel()
        await asyncio.gather(change, lost, return_exceptions=True)
    error = None if change.cancelled() else change.exception()
    if error is not None:
        raise error


async def _arm_state(client: StationClient, arm: str) -> ArmState | None:
    return next(
        (snapshot for snapshot in (await client.state()).arms if snapshot.arm == arm),
        None,
    )


def _arm_interlocked(arm_state: ArmState) -> bool:
    return any(interlock.kind in ARM_INTERLOCKS for interlock in arm_state.interlocks)


async def wait_for_arm_recovery(client: StationClient, arm: str) -> bool:
    """Wait until the arm holds with no arm interlock; false once it is disabled
    or faulted instead. The next observation replaces the old state."""
    while True:
        if client.lease_lost.is_set():
            raise LeaseLost
        observed = client.change_stamp()
        arm_state = await _arm_state(client, arm)
        if arm_state is not None and arm_state.mode in {
            ArmMode.faulted,
            ArmMode.disabled,
        }:
            return False
        if (
            arm_state is not None
            and arm_state.mode is ArmMode.holding
            and not _arm_interlocked(arm_state)
        ):
            return True
        await _wait_for_change_or_lease_loss(client, observed)


async def capture_when_ready(
    robot: RobotTools, note: str = "", extra_lines: tuple[str, ...] = ()
) -> tuple[CapturedObservation, int]:
    """Wait for required resources and capture a fresh observation, returning it
    with the arm's revision it is valid for; send that as `expected_revision`."""
    missing_cameras: set[str] = set()
    camera_retry_seconds = INITIAL_CAMERA_RETRY_SECONDS
    while True:
        if robot.station.lease_lost.is_set():
            raise LeaseLost
        try:
            before = await _arm_state(robot.station, robot.arm)
            if before is None or before.mode in {ArmMode.faulted, ArmMode.disabled}:
                raise ArmRecoveryEnded
            if _arm_interlocked(before):
                if not await wait_for_arm_recovery(robot.station, robot.arm):
                    raise ArmRecoveryEnded
                continue
            captured = await robot.capture(note, extra_lines)
            after = await _arm_state(robot.station, robot.arm)
            if after is None or after.mode in {ArmMode.faulted, ArmMode.disabled}:
                raise ArmRecoveryEnded
            if after.revision != before.revision:
                continue
            break
        except RequiredCameraUnavailable as error:
            if error.camera not in missing_cameras:
                missing_cameras.add(error.camera)
                log.info("Waiting for required camera: %s", error.camera)
                try:
                    await robot.lease.report_camera(error.camera, False)
                except (
                    LeaseLost,
                    NativeCommandRefused,
                    HttpError,
                    ConnectionError,
                    TimeoutError,
                ) as report_error:
                    log.warning("Could not report the camera missing: %s", report_error)
            await asyncio.sleep(camera_retry_seconds)
            camera_retry_seconds = min(
                camera_retry_seconds * 2, MAX_CAMERA_RETRY_SECONDS
            )
        except ArmCommunicationInterrupted:
            if not await wait_for_arm_recovery(robot.station, robot.arm):
                raise ArmRecoveryEnded from None
    missing_cameras.update(
        interlock.camera
        for interlock in after.interlocks
        if interlock.kind == "required_camera_unavailable"
        and interlock.camera in robot.cameras
    )
    # WHY: A capture just succeeded, so every required camera gave a fresh frame.
    for camera in missing_cameras:
        try:
            await robot.lease.report_camera(camera, True)
        except (
            LeaseLost,
            NativeCommandRefused,
            HttpError,
            ConnectionError,
            TimeoutError,
        ) as report_error:
            log.warning("Could not report camera recovery: %s", report_error)
    return captured, after.revision
