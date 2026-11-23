import asyncio
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

from hebbian_rua.client.contract.station_api import (
    ArmMode,
    CommandCompletion,
    CommandOutcomeUnknown,
    CommandRecord,
    CommandRefused,
    ControlAction,
    FinishedCommand,
    IkFailure,
    KnownCommandOutcome,
    LeaseHolder,
    PathLimited,
    PendingCommand,
    PoseLimited,
    RecordedControl,
    RecordedMove,
    RejectionReason,
)
from hebbian_rua.client.rotations import quaternion_from_roll_pitch_yaw
from hebbian_rua.client.station_client import (
    CommandResult,
    ResultOutcome,
    StationClient,
)
from hebbian_rua.client.station_types import CameraUnavailable
from tests.fixture_station import ARM, running_station


def test_close_releases_http_clients_when_connection_close_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def exercise() -> None:
        client = StationClient("http://127.0.0.1:9")

        async def fail_close() -> None:
            raise RuntimeError("connection close failed")

        monkeypatch.setattr(client._connection, "close", fail_close)
        with pytest.raises(RuntimeError, match="connection close failed"):
            await client.close()
        assert client._api._station.is_closed
        assert client._api._media.is_closed

    asyncio.run(exercise())


@pytest.mark.server
def test_enable_move_and_stop_round_trip(server_binary: Path) -> None:
    with running_station(server_binary, enabled=False) as running:

        async def exercise() -> None:
            async with (
                StationClient(running.server) as client,
                client.holding_lease("round trip", holder=LeaseHolder.manual) as lease,
            ):
                assert lease.status.holder is LeaseHolder.manual
                arm = client.arm(ARM)
                assert (await arm.enable()).completed
                state = await arm.state()
                assert state.mode == ArmMode.holding
                joints = list(state.measured_joints)
                joints[0] += 0.02
                moved = await lease.arm(ARM).move_to_joints(joints)
                assert moved.completed, moved
                assert isinstance(moved.record, CommandRecord)
                assert moved.record.phase.kind == "finished"
                assert (
                    moved.record.phase.completion.outcome
                    is KnownCommandOutcome.completed
                )
                assert (await arm.stop()).completed

        asyncio.run(exercise())


def test_an_unreachable_station_fails_on_the_first_call(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "hebbian_rua.client.station_connection.CONNECT_TIMEOUT_SECONDS", 0.5
    )

    async def exercise() -> None:
        async with StationClient("http://127.0.0.1:9") as client:
            with pytest.raises(ConnectionError, match=r"127\.0\.0\.1:9"):
                await client.state()

    asyncio.run(exercise())


def test_rpy_quaternion_matches_known_rotations() -> None:
    half = 2**-0.5
    for angles, expected in [
        ((0, 0, 0), [0, 0, 0, 1]),
        ((3.141592653589793 / 2, 0, 0), [half, 0, 0, half]),
        ((0, 0, 3.141592653589793 / 2), [0, 0, half, half]),
        # INVARIANT: Roll then yaw about the world axes: fixes the composition order.
        ((3.141592653589793 / 2, 0, 3.141592653589793 / 2), [0.5, 0.5, 0.5, 0.5]),
    ]:
        assert quaternion_from_roll_pitch_yaw(*angles) == pytest.approx(expected)


def test_a_command_result_reports_how_its_record_or_refusal_ended() -> None:
    unfinished = CommandRecord(
        command_id="c",
        label="enable",
        accepted_at=0.0,
        checkpoints=[],
        command=RecordedControl(action=ControlAction.enable),
        phase=PendingCommand(),
    )
    assert CommandResult.of(unfinished).outcome is ResultOutcome.UNFINISHED
    refused = CommandRefused(
        reason=RejectionReason.not_enabled, detail="enable it first"
    )
    rejected = CommandResult.of(refused)
    assert rejected.outcome is ResultOutcome.REJECTED
    assert not rejected.completed
    assert "not_enabled" in (rejected.detail or "")
    for outcome, expected in [
        (KnownCommandOutcome.tripped, ResultOutcome.TRIPPED),
        (KnownCommandOutcome.completed, ResultOutcome.COMPLETED),
    ]:
        terminal = unfinished.model_copy(
            update={
                "phase": FinishedCommand(
                    completion=CommandCompletion(finished_at=1.0, outcome=outcome),
                )
            }
        )
        result = CommandResult.of(terminal)
        assert result.outcome is expected
        assert result.completed == (expected is ResultOutcome.COMPLETED)
    interrupted = unfinished.model_copy(
        update={
            "phase": CommandOutcomeUnknown(
                finished_at=1.0, detail="Controller connection lost"
            ),
        }
    )
    result = CommandResult.of(interrupted)
    assert result.outcome.value == "interrupted"
    assert not result.completed
    assert result.detail == "Controller connection lost"


def test_command_result_distinguishes_completed_and_limited_plans() -> None:
    completed = CommandRecord(
        command_id="move",
        label="move_to_pose",
        accepted_at=0.0,
        checkpoints=[],
        command=RecordedMove(destination_joints=[0.2], joint_speed=1.0),
        phase=FinishedCommand(
            completion=CommandCompletion(
                finished_at=1.0, outcome=KnownCommandOutcome.completed
            )
        ),
    )
    assert CommandResult.of(completed).completed
    assert not CommandResult.of(completed).was_limited
    limits = [
        {"path_limited": PathLimited(reason="self-collision")},
        {
            "pose_limited": PoseLimited(
                advice="adjust the target",
                orientation_shortfall_radians=0.0,
                position_shortfall_meters=0.01,
                reason=IkFailure.out_of_reach,
                waypoint=0,
            )
        },
    ]
    for limit in limits:
        assert isinstance(completed.command, RecordedMove)
        record = completed.model_copy(
            update={"command": completed.command.model_copy(update=limit)}
        )
        result = CommandResult.of(record)
        assert result.completed
        assert result.was_limited
    refused = CommandResult.of(
        CommandRefused(reason=RejectionReason.not_enabled, detail="enable it first")
    )
    assert not refused.completed
    assert not refused.was_limited


class _MediaWithOneStalledCamera(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/cameras":
            body = b'[{"name": "wrist", "available": false}]'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
        elif self.path == "/viewer/api/media-status":
            body = (
                b'{"service":{"state":"running"},"cameras":{"wrist":{"state":"stalled"},'
                b'"front":{"state":"capturing","publication":{"state":"publishing"}}}}'
            )
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
        else:
            body = b""
            self.send_response(404)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def media_url() -> Iterator[str]:
    server = HTTPServer(("127.0.0.1", 0), _MediaWithOneStalledCamera)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{server.server_port}"
    server.shutdown()
    server.server_close()


def test_a_camera_without_a_frame_names_the_cameras_the_media_service_lists(
    media_url: str,
) -> None:
    async def exercise() -> None:
        # WHY: One stub plays both the station (media status) and the media service.
        async with StationClient(media_url, media_server=media_url) as client:
            with pytest.raises(CameraUnavailable) as failure:
                await client.camera_jpeg("wrist")
            assert failure.value.name == "wrist"
            assert [camera.name for camera in failure.value.listed] == ["wrist"]
            assert "wrist (unavailable)" in str(failure.value)
            assert failure.value.last_error == "Local camera capture stalled. Retrying"
            assert "last error: Local camera capture stalled. Retrying" in str(
                failure.value
            )

    asyncio.run(exercise())
