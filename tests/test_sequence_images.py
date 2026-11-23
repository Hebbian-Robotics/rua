import asyncio
from dataclasses import replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread

import pytest

from hebbian_rua.client.llm.response_format import render_output, select_output_cameras
from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.types import (
    ModelContent,
    RobotOutput,
    RobotToolCall,
)
from hebbian_rua.client.station_client import StationClient
from tests.fixture_station import ARM, running_station


class CameraHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/cameras/missing":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "image/jpeg")
        self.end_headers()
        self.wfile.write(
            b"x" * 4096
            if self.path == "/cameras/oversized"
            else b"[]"
            if self.path == "/cameras"
            else b"jpeg"
        )

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.mark.server
def test_missing_camera_does_not_change_sequence_outcome_or_drop_other_images(
    server_binary: Path,
) -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), CameraHandler)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()

    async def exercise(station_url: str) -> None:
        async with (
            StationClient(
                station_url, media_server=f"http://127.0.0.1:{server.server_port}"
            ) as station,
            station.holding_lease("sequence images") as lease,
        ):
            robot = RobotTools(
                station, lease, ARM, optional_cameras=["missing", "front"]
            )
            missing = await robot.execute_one(
                RobotToolCall("missing-history", "observe", {"source": "last_sequence"})
            )
            assert missing.is_error
            assert missing.cameras == {}
            result = await robot.execute_one(
                RobotToolCall(
                    "move",
                    "action_sequence",
                    {"steps": [{"gripper_opening": 1.0}, {"gripper_opening": 0.0}]},
                )
            )
            assert not result.is_error
            history = robot.sequence_images()
            assert history is not None
            assert history.missed_samples > 0
            assert any(image.camera == "front" for image in history.images)
            assert history.images[-1].boundary == "end"
            mapped = robot.with_output_formatter(
                lambda output: render_output(select_output_cameras(output, ["front"]))
            )
            automatic = await mapped.observe()
            assert set(automatic.cameras) == {"front"}
            for attempt in range(2):
                review = await mapped.execute_one(
                    RobotToolCall(
                        f"review-{attempt}", "observe", {"source": "last_sequence"}
                    )
                )
                assert not review.is_error
                assert isinstance(review.data, dict)
                assert review.data["source"] == "last_sequence"
                assert review.data["sequence_call_id"] == "move"
                assert review.cameras
                assert all(name.startswith("Sequence move") for name in review.cameras)
                assert all(jpeg == b"jpeg" for jpeg in review.cameras.values())
                assert set((await mapped.observe()).cameras) == {"front"}

            def without_history_images(output: RobotOutput) -> ModelContent:
                if output.sequence_images is not None:
                    output = replace(
                        output,
                        sequence_images=replace(output.sequence_images, images=[]),
                    )
                return render_output(output)

            hidden = await robot.with_output_formatter(
                without_history_images
            ).execute_one(
                RobotToolCall("hidden", "observe", {"source": "last_sequence"})
            )
            assert hidden.cameras == {}
            current = await mapped.execute_one(RobotToolCall("observe", "observe", {}))
            assert not current.is_error
            assert set(current.cameras) == {"front"}

            def broken_formatter(_output: RobotOutput) -> ModelContent:
                raise RuntimeError("presentation failed")

            with pytest.raises(RuntimeError, match="presentation failed"):
                await robot.with_output_formatter(broken_formatter).execute_one(
                    RobotToolCall("broken", "observe", {})
                )

    try:
        with running_station(
            server_binary, media_address=f"127.0.0.1:{server.server_port}"
        ) as station:
            asyncio.run(exercise(station.server))
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_camera_byte_limit_rejects_oversized_content_before_retaining_it() -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), CameraHandler)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()

    async def exercise() -> None:
        async with StationClient(
            media_server=f"http://127.0.0.1:{server.server_port}"
        ) as station:
            with pytest.raises(ValueError, match="maximum"):
                await station.camera_jpeg("oversized", maximum_bytes=1024)
            assert (await station.camera_jpeg("front", maximum_bytes=4)).jpeg == b"jpeg"

    try:
        asyncio.run(exercise())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
