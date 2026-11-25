"""Interlock recovery and revision-fenced observation for robot-use agents.

An agent holds the lease, observes, and sends commands carrying the arm's
`revision` from that observation, so a command planned before an interruption
is refused as `stale_revision` instead of acting on an out-of-date view. See
docs/control-model.md.
"""

import asyncio

from hebbian_rua.client.contract.station_api import ArmMode, ArmState
from hebbian_rua.client.llm.robot_tools import (
    ArmRecoveryEnded as ArmRecoveryEnded,
)
from hebbian_rua.client.llm.robot_tools import (
    CapturedObservation,
    ObservationWaitTimedOut,
    RobotTools,
)
from hebbian_rua.client.station_client import StationClient
from hebbian_rua.client.station_types import LeaseLost

ARM_INTERLOCKS = frozenset({"arm_disconnected", "recovery_approval_required"})


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
        if client.lease_lost.is_set():
            raise LeaseLost
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
    while True:
        if robot.station.lease_lost.is_set():
            raise LeaseLost
        try:
            captured = await robot.capture(note, extra_lines, wait_milliseconds=5000)
        except ObservationWaitTimedOut:
            continue
        if robot.station.lease_lost.is_set():
            raise LeaseLost
        captured_revision = captured.observation.revisions[robot.arm]
        latest = await _arm_state(robot.station, robot.arm)
        if robot.station.lease_lost.is_set():
            raise LeaseLost
        # INVARIANT: Ignore captures already superseded by the live state feed.
        if latest is None or latest.revision <= captured_revision:
            return captured, captured_revision
