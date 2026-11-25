import asyncio
from base64 import b64encode
from pathlib import Path

import pytest

from hebbian_rua import RobotTools, StationClient
from hebbian_rua.client.contract.station_api import ObservationRequest
from hebbian_rua.client.llm.recovery import capture_when_ready
from hebbian_rua.client.station_types import LeaseLost
from tests.fixture_station import ARM, running_station
from tests.fixture_station_http import scripted_http_station, state_message


@pytest.mark.parametrize("revision_changes_during_capture", [True, False])
def test_recovery_discards_a_capture_from_an_old_arm_revision(
    revision_changes_during_capture: bool,
) -> None:
    async def exercise() -> None:
        first_capture_started = asyncio.Event()
        release_first_capture = asyncio.Event()

        async def capture_observation() -> object:
            if first_capture_started.is_set():
                revision, image = 1, b"fresh frame"
            else:
                revision, image = 0, b"stale frame"
                first_capture_started.set()
                await release_first_capture.wait()
            return {
                "status": "observation_captured",
                "state": state_message(holder="agent", revision=revision)["state"],
                "cameras": {
                    "wrist": {
                        "jpeg_base64": b64encode(image).decode(),
                        "retrieved_between_unix_seconds": [10.0, 10.1],
                    }
                },
                "missing_optional_cameras": [],
            }

        async with scripted_http_station(
            capture_observation=capture_observation
        ) as station:
            station.grant_lease.set()
            async with (
                StationClient(station.url) as client,
                client.holding_lease("recovery test") as lease,
                asyncio.timeout(5),
                asyncio.TaskGroup() as tasks,
            ):
                robot = RobotTools(client, lease, "follower", cameras=["wrist"])

                async def publish_current_revision() -> None:
                    await station.event("station", state_message(revision=1))
                    await client.wait_for_state(
                        lambda state: state.arms[0].revision == 1
                    )

                if not revision_changes_during_capture:
                    await publish_current_revision()
                capture = tasks.create_task(capture_when_ready(robot))
                try:
                    await first_capture_started.wait()
                    # WHY: Synchronize both transports so the stale response arrives last.
                    if revision_changes_during_capture:
                        await publish_current_revision()
                finally:
                    release_first_capture.set()
                captured, revision = await capture
                assert revision == 1
                assert captured.observation.revisions == {"follower": 1}
                assert captured.captures["wrist"].jpeg == b"fresh frame"

    asyncio.run(exercise())


@pytest.mark.server
def test_waiting_for_observation_times_out_and_detects_lease_takeover(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            selection = ObservationRequest(
                arms=[ARM], required_cameras=["unavailable_test_camera"]
            )
            async with (
                StationClient(station.server) as client,
                StationClient(station.server) as replacement,
                client.holding_lease(
                    "observation wait", required_cameras=["unavailable_test_camera"]
                ) as lease,
                asyncio.timeout(10),
            ):
                timed_out = await lease.wait_for_observation(selection, ARM, 100)
                assert timed_out.root.status == "observation_wait_timed_out"
                async with asyncio.TaskGroup() as tasks:

                    async def wait_until_lease_lost() -> None:
                        with pytest.raises(LeaseLost):
                            await lease.wait_for_observation(selection, ARM, 5000)

                    tasks.create_task(wait_until_lease_lost())
                    await client.wait_for_state(
                        lambda state: bool(state.arms[0].interlocks)
                    )
                    async with replacement.holding_lease("replacement", take=True):
                        pass

        asyncio.run(exercise())
