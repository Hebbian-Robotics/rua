import json
import os
import subprocess
from collections.abc import Generator, Iterator
from contextlib import contextmanager
from pathlib import Path

import httpx2
import pytest

from tests.fixture_station import REPOSITORY_ROOT, running_station
from tests.fixture_wait import wait_until

VIEWER_COMMAND = [
    "node",
    "--import",
    "tsx",
    "--conditions=development",
    "examples/typescript/viewer/server/src/main.ts",
]


@pytest.mark.typescript
def test_viewer_requires_a_model_provider_key() -> None:
    result = subprocess.run(
        VIEWER_COMMAND,
        env={
            **os.environ,
            "OPENAI_API_KEY": "",
            "ANTHROPIC_API_KEY": " ",
            "GEMINI_API_KEY": "",
            "GOOGLE_API_KEY": "",
            "AGENT_PORT": "0",
        },
        cwd=REPOSITORY_ROOT,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.returncode == 1, result.stderr
    assert "OPENAI_API_KEY" in result.stderr
    assert "--manual-only" in result.stderr


@contextmanager
def viewer_backend(station_url: str, run_directory: Path) -> Generator[str]:
    environment = {
        **os.environ,
        "HEBBIAN_STATION_URL": station_url,
        "AGENT_HOST": "127.0.0.1",
        "AGENT_PORT": "0",
        "AGENT_RUNS_DIR": str(run_directory),
    }
    with subprocess.Popen(
        [*VIEWER_COMMAND, "--manual-only"],
        env=environment,
        cwd=REPOSITORY_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    ) as process:
        try:
            assert process.stdout is not None
            for line in process.stdout:
                if line.startswith("Viewer on "):
                    yield line.split()[2].rstrip(",")
                    return
            pytest.fail("Viewer backend exited before announcing its address")
        finally:
            process.terminate()
            process.wait(timeout=10)


def ready_session(lines: Iterator[str]) -> str:
    session_id = None
    for line in lines:
        if not line.startswith("data: "):
            continue
        event = json.loads(line.removeprefix("data: "))
        if event["kind"] == "session":
            session_id = event["id"]
        if event["kind"] == "state" and event["state"]["arms"]:
            assert isinstance(session_id, str)
            return session_id
    pytest.fail("Viewer stream ended before it reported station state")


@pytest.mark.typescript
def test_manual_sessions_release_control_on_disconnect(
    server_binary: Path, tmp_path: Path
) -> None:
    with (
        running_station(server_binary, enabled=False) as station,
        viewer_backend(station.server, tmp_path) as viewer,
        httpx2.Client(base_url=viewer, timeout=10) as client,
        client.stream("GET", "/api/manual/events") as observer,
    ):
        capabilities = client.get("/api/control/capabilities")
        capabilities.raise_for_status()
        assert capabilities.json()["manual_pose_control"] is False
        recordings = client.get("/api/recordings")
        recordings.raise_for_status()
        assert recordings.json()["recordings"] == []

        observer_lines = observer.iter_lines()
        observer_id = ready_session(observer_lines)

        def request(session: str, **body: object) -> httpx2.Response:
            return client.post(
                "/api/manual/request", headers={"x-viewer-session": session}, json=body
            )

        with client.stream("GET", "/api/manual/events") as operator:
            operator_lines = operator.iter_lines()
            operator_id = ready_session(operator_lines)
            grant = request(operator_id, kind="acquire", label="operator")
            assert grant.status_code == 200, grant.text
            lease_id = grant.json()["leaseId"]
            assert (
                request(observer_id, kind="return", leaseId=lease_id).status_code == 409
            )
            assert (
                request(observer_id, kind="acquire", label="observer").status_code
                == 409
            )
            assert station.get("/state").json()["lease"]["label"] == "operator"

        wait_until(
            lambda: station.get("/state").json()["lease"] is None,
            timeout=5,
            interval=0.05,
            message="Closing the tab did not release its lease",
        )
        request(observer_id, kind="acquire", label="observer").raise_for_status()
        assert request(operator_id, kind="heartbeat").status_code == 409
