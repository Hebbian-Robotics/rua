import json
from collections.abc import Iterator
from pathlib import Path

import httpx2
import pytest

from tests.fixture_station import running_station


def read_event(lines: Iterator[str], name: str) -> tuple[str, dict[str, object]]:
    event = ""
    cursor = ""
    for line in lines:
        if line.startswith("event: "):
            event = line.removeprefix("event: ")
        elif line.startswith("id: "):
            cursor = line.removeprefix("id: ")
        elif event == name and line.startswith("data: "):
            return cursor, json.loads(line.removeprefix("data: "))
        elif not line:
            event = ""
            cursor = ""
    raise AssertionError(f"The stream closed before delivering a {name} event")


@pytest.mark.server
def test_expired_run_cursor_can_resume_live_state(server_binary: Path) -> None:
    with running_station(server_binary, enabled=False) as station:
        with httpx2.stream(
            "GET", f"{station.server}/events", headers={"X-RUA-Run-Cursor": "latest"}
        ) as stream:
            state_cursor, snapshot = read_event(stream.iter_lines(), "snapshot")
            instance = snapshot["station_instance_id"]
            run_cursor = snapshot["run_sequence"]

        span = {
            "trace_id": "test-trace",
            "span_id": "test-span",
            "name": "test span",
            "start_time_unix_nano": "1",
            "end_time_unix_nano": "2",
        }
        httpx2.post(
            f"{station.server}/run/spans", json={"spans": [span] * 4200}, timeout=10
        ).raise_for_status()
        with httpx2.stream(
            "GET",
            f"{station.server}/events",
            headers={
                "Last-Event-ID": state_cursor,
                "X-RUA-Run-Cursor": f"{instance}:{run_cursor}",
            },
            timeout=10,
        ) as stream:
            stream.raise_for_status()
            lines = stream.iter_lines()
            _, gap = read_event(lines, "run")
            assert gap["kind"] == "run_resync"
            assert gap["newest"] == 4200
            _, run = read_event(lines, "run")
            assert run["kind"] == "run"


@pytest.mark.server
def test_station_restart_rejects_old_run_epoch_even_at_the_same_sequence(
    server_binary: Path,
) -> None:
    with running_station(server_binary, enabled=False) as first:
        old_instance = first.get("/instance").json()["id"]
    with (
        running_station(server_binary, enabled=False) as replacement,
        httpx2.stream(
            "GET",
            f"{replacement.server}/events",
            headers={"X-RUA-Run-Cursor": f"{old_instance}:0"},
        ) as stream,
    ):
        stream.raise_for_status()
        lines = stream.iter_lines()
        _, snapshot = read_event(lines, "snapshot")
        assert snapshot["station_instance_id"] != old_instance
        assert snapshot["run_sequence"] == 0
        _, run = read_event(lines, "run")
        assert run["kind"] == "run_resync"
        assert run["newest"] == 0


@pytest.mark.server
def test_interrupted_run_replay_resumes_after_last_delivered_record(
    server_binary: Path,
) -> None:
    with running_station(server_binary, enabled=False) as station:
        for note in ["first", "second", "third"]:
            httpx2.post(
                f"{station.server}/run/events",
                json={"type": "notes_update", "notes": [note]},
            ).raise_for_status()
        with httpx2.stream(
            "GET", f"{station.server}/events", headers={"X-RUA-Run-Cursor": "0"}
        ) as stream:
            stream.raise_for_status()
            lines = stream.iter_lines()
            state_cursor, snapshot = read_event(lines, "snapshot")
            _, delivered = read_event(lines, "run")
            assert delivered["seq"] == 1
            assert delivered["event"] == {"type": "notes_update", "notes": ["first"]}

        instance = snapshot["station_instance_id"]
        with httpx2.stream(
            "GET",
            f"{station.server}/events",
            headers={
                "Last-Event-ID": state_cursor,
                "X-RUA-Run-Cursor": f"{instance}:{delivered['seq']}",
            },
        ) as stream:
            stream.raise_for_status()
            lines = stream.iter_lines()
            for expected_sequence, note in [(2, "second"), (3, "third")]:
                _, replayed = read_event(lines, "run")
                assert replayed["seq"] == expected_sequence
                assert replayed["event"] == {"type": "notes_update", "notes": [note]}
