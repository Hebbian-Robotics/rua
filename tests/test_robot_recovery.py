import asyncio
from base64 import b64encode

from hebbian_rua import RobotTools, StationClient
from hebbian_rua.client.llm.recovery import capture_when_ready
from tests.fixture_station_http import scripted_http_station, state_message


def test_recovery_discards_a_capture_when_the_arm_revision_changes() -> None:
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
                capture = tasks.create_task(capture_when_ready(robot))
                try:
                    await first_capture_started.wait()
                    # WHY: Synchronize both transports so the stale response arrives last.
                    await station.event("station", state_message(revision=1))
                    await client.wait_for_state(
                        lambda state: state.arms[0].revision == 1
                    )
                finally:
                    release_first_capture.set()
                captured, revision = await capture
                assert revision == 1
                assert captured.observation.revisions == {"follower": 1}
                assert captured.captures["wrist"].jpeg == b"fresh frame"

    asyncio.run(exercise())
