import os
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread

import pytest

from tests.fixture_station import running_station


@pytest.mark.server
@pytest.mark.typescript
def test_typescript_robot_tools_use_the_simulated_station(server_binary: Path) -> None:
    repository_directory = Path(__file__).resolve().parent.parent
    with running_station(server_binary) as station:
        subprocess.run(
            [
                "pnpm",
                "exec",
                "tsx",
                "--conditions=development",
                "typescript-client/tests/simulated-robot-tools.ts",
                station.server,
            ],
            cwd=repository_directory,
            check=True,
            timeout=45,
        )


@pytest.mark.server
@pytest.mark.typescript
def test_packed_typescript_client_observes_the_station(server_binary: Path) -> None:
    class CameraHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            if self.path == "/cameras":
                body = b'[{"name":"sim_overview","available":true}]'
                content_type = "application/json"
            elif self.path == "/cameras/sim_overview":
                body = b"\xff\xd8\xff\xd9"
                content_type = "image/jpeg"
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    repository_directory = Path(__file__).resolve().parent.parent
    with ThreadingHTTPServer(("127.0.0.1", 0), CameraHandler) as camera_server:
        camera_thread = Thread(target=camera_server.serve_forever, daemon=True)
        camera_thread.start()
        try:
            with running_station(
                server_binary, media_address=f"127.0.0.1:{camera_server.server_port}"
            ) as station:
                subprocess.run(
                    ["pnpm", "test:package"],
                    cwd=repository_directory,
                    env=os.environ | {"RUA_PACKAGE_CHECK_STATION": station.server},
                    check=True,
                    timeout=120,
                )
        finally:
            camera_server.shutdown()
            camera_thread.join()
