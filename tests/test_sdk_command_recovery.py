import asyncio
import json
import socket
import subprocess
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Lock, Thread
from typing import Literal
from urllib.parse import urlsplit

import pytest

from hebbian_rua.client.contract.station_api import (
    ActionSequenceCompleted,
    ActionSequenceGripper,
    CommandRecord,
)
from hebbian_rua.client.station_client import StationClient
from tests.fixture_station import ARM, REPOSITORY_ROOT, running_station


async def run_python_operation(
    station_url: str, operation: Literal["command", "action_sequence"]
) -> str:
    async with (
        StationClient(station_url) as client,
        client.holding_lease("lost reply test") as lease,
    ):
        arm = lease.arm(ARM)
        if operation == "action_sequence":
            sequence = await arm.run_action_sequence(
                [ActionSequenceGripper(gripper_opening=1)]
            )
            assert isinstance(sequence.phase, ActionSequenceCompleted)
            assert len(sequence.steps) == 1
            return sequence.id
        result = await arm.set_gripper(1)
        assert result.completed, result
        assert isinstance(result.record, CommandRecord)
        state = await client.arm(ARM).state()
        assert state.last_command is not None
        assert state.last_command.command_id == result.record.command_id
        assert state.gripper_opening == pytest.approx(1, abs=0.02)
        return result.record.command_id


@pytest.mark.server
@pytest.mark.parametrize(
    "client",
    ["python", pytest.param("typescript", marks=pytest.mark.typescript)],
)
@pytest.mark.parametrize("operation", ["command", "action_sequence"])
def test_sdk_recovers_the_original_operation_after_losing_its_reply(
    server_binary: Path, client: str, operation: Literal["command", "action_sequence"]
) -> None:
    submission_path = "/commands" if operation == "command" else "/action_sequence"
    with running_station(server_binary) as station:
        upstream_address = urlsplit(station.server)
        submissions: list[tuple[str, str]] = []
        submissions_lock = Lock()

        class LoseFirstOperationReply(BaseHTTPRequestHandler):
            def forward(self) -> None:
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                headers = {
                    name: self.headers[name]
                    for name in ("Content-Type", "Last-Event-ID", "X-RUA-Run-Cursor")
                    if name in self.headers
                }
                upstream = HTTPConnection(
                    upstream_address.hostname or "127.0.0.1",
                    upstream_address.port,
                    timeout=30,
                )
                try:
                    upstream.request(
                        self.command, self.path, body=body, headers=headers
                    )
                    response = upstream.getresponse()
                    if (
                        self.command == "POST"
                        and self.path == submission_path
                        and response.status == 200
                    ):
                        answer_body = response.read()
                        request = json.loads(body)
                        answer = json.loads(answer_body)
                        if operation == "command":
                            operation_id = request["operation_id"]
                            result_id = answer["records"][0]["record"]["command_id"]
                        else:
                            operation_id = request["request_id"]
                            result_id = answer
                        with submissions_lock:
                            drop_reply = not submissions
                            submissions.append((operation_id, result_id))
                        if drop_reply:
                            self.connection.shutdown(socket.SHUT_RDWR)
                            return
                        self.send_response(response.status)
                        self.send_header("Content-Type", "application/json")
                        self.end_headers()
                        self.wfile.write(answer_body)
                        return

                    self.send_response(response.status)
                    for name in ("Content-Type", "X-RUA-Run-Cursor"):
                        if value := response.getheader(name):
                            self.send_header(name, value)
                    self.end_headers()
                    while chunk := response.read1(64 * 1024):
                        self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                finally:
                    upstream.close()

            do_GET = forward
            do_POST = forward

        with ThreadingHTTPServer(("127.0.0.1", 0), LoseFirstOperationReply) as proxy:
            proxy_thread = Thread(target=proxy.serve_forever)
            proxy_thread.start()
            try:
                proxy_url = f"http://127.0.0.1:{proxy.server_port}"
                if client == "python":
                    result_id = asyncio.run(
                        asyncio.wait_for(
                            run_python_operation(proxy_url, operation), timeout=15
                        )
                    )
                else:
                    run = subprocess.run(
                        [
                            "pnpm",
                            "exec",
                            "tsx",
                            "--conditions=development",
                            "typescript-client/tests/simulated-command-recovery.ts",
                            proxy_url,
                            ARM,
                            operation,
                        ],
                        cwd=REPOSITORY_ROOT,
                        stdout=subprocess.PIPE,
                        text=True,
                        check=True,
                        timeout=15,
                    )
                    result_id = run.stdout
            finally:
                proxy.shutdown()
                proxy_thread.join()

        assert len(submissions) >= 2
        assert set(submissions) == {(submissions[0][0], result_id)}
        if operation == "action_sequence":
            assert result_id == submissions[0][0]
