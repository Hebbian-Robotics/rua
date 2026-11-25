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


@contextmanager
def viewer_backend(
    station_url: str, run_directory: Path, *, manual_only: bool = True
) -> Generator[str]:
    environment = {
        **os.environ,
        "HEBBIAN_STATION_URL": station_url,
        "AGENT_HOST": "127.0.0.1",
        "AGENT_PORT": "0",
        "AGENT_RUNS_DIR": str(run_directory),
    }
    arguments = (
        [*VIEWER_COMMAND, "--manual-control-only-no-agent"]
        if manual_only
        else VIEWER_COMMAND
    )
    with subprocess.Popen(
        arguments,
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


@pytest.mark.server
@pytest.mark.typescript
def test_viewer_reads_provider_keys_from_the_process_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai")
    monkeypatch.setenv("GEMINI_API_KEY", "")
    monkeypatch.setenv("GOOGLE_API_KEY", "")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-anthropic")
    with viewer_backend("http://127.0.0.1:1", tmp_path, manual_only=False) as viewer:
        response = httpx2.get(f"{viewer}/api/model-providers", timeout=10)
        response.raise_for_status()
        assert {entry["provider"]: entry["available"] for entry in response.json()} == {
            "openai": True,
            "gemini": False,
            "anthropic": True,
        }


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
    server_binary: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai")
    with (
        running_station(server_binary, enabled=False) as station,
        viewer_backend(station.server, tmp_path, manual_only=False) as viewer,
        httpx2.Client(base_url=viewer, timeout=10) as client,
        client.stream("GET", "/api/manual/events") as observer,
    ):
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


@pytest.mark.typescript
def test_recording_controls_reject_cross_site_form_posts(
    server_binary: Path, tmp_path: Path
) -> None:
    with (
        running_station(server_binary, enabled=False) as station,
        viewer_backend(station.server, tmp_path) as viewer,
        httpx2.Client(base_url=viewer, timeout=10) as client,
    ):
        for route in ("/api/recording/start", "/api/recording/stop"):
            assert (
                client.post(
                    route,
                    headers={
                        "Origin": "https://example.invalid",
                        "Sec-Fetch-Site": "cross-site",
                        "Content-Type": "application/x-www-form-urlencoded",
                    },
                    content="",
                ).status_code
                == 403
            )
        assert (
            client.post("/api/recording/stop", headers={"Origin": viewer}).status_code
            == 200
        )


@pytest.mark.typescript
def test_viewer_abort_stops_an_admitted_move(
    server_binary: Path, tmp_path: Path
) -> None:
    with (
        running_station(server_binary) as station,
        viewer_backend(station.server, tmp_path) as viewer,
    ):
        subprocess.run(
            [
                "node",
                "--experimental-eventsource",
                "--import",
                "tsx",
                "--conditions=development",
                "examples/typescript/viewer/tests/simulated-viewer-cancellation.ts",
                viewer,
                station.server,
            ],
            cwd=REPOSITORY_ROOT,
            check=True,
            timeout=20,
        )
