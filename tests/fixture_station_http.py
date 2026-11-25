import asyncio
import json
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from typing import Any

STATION_ID = "11111111-1111-4111-8111-111111111111"


def command_record(
    command_id: str, finished_unix_nano: str | None = None
) -> dict[str, Any]:
    return {
        "command_id": command_id,
        "label": "enable",
        "accepted_unix_nano": "1000000000",
        "command": {"kind": "control", "action": "enable"},
        "phase": {"kind": "pending"}
        if finished_unix_nano is None
        else {
            "kind": "finished",
            "completion": {
                "outcome": "completed",
                "finished_unix_nano": finished_unix_nano,
                "detail": None,
            },
        },
        "checkpoints": [],
        "trace_context": None,
    }


def record_frame(arm: str, seq: int, record: dict[str, Any]) -> dict[str, Any]:
    return {
        "kind": "frame",
        "arm": arm,
        "frame": {
            "seq": seq,
            "emitted_unix_nano": "1",
            "event": {**record, "kind": "record"},
        },
    }


def state_message(
    *,
    arm: str = "follower",
    mode: str = "holding",
    sampled_at: float | None = None,
    active: str | None = None,
    last: str | None = None,
    holder: str | None = None,
    revision: int = 0,
) -> dict[str, Any]:
    return {
        "kind": "state",
        "state": {
            "arms": [
                {
                    "active_command": None
                    if active is None
                    else command_record(active),
                    "arm": arm,
                    "commanded_joints": None,
                    "controller": "ik",
                    "fault": None,
                    "gripper_opening": None,
                    "last_command": None
                    if last is None
                    else command_record(last, "2000000000"),
                    "measured_joint_efforts": [],
                    "measured_joint_velocities": [],
                    "measured_joints": [],
                    "mode": mode,
                    "recovery_issue": None,
                    "rest_pose": None,
                    "sampled_unix_nano": None
                    if sampled_at is None
                    else str(round(sampled_at * 1_000_000_000)),
                    "segment": None,
                    "tool": None,
                    "interlocks": [],
                    "revision": revision,
                }
            ],
            "lease": None
            if holder is None
            else {
                "holder": holder,
                "label": "x",
                "required_cameras": [],
                "on_interruption": "wait_for_approval",
                "expires_in_seconds": 15,
            },
            "description": None,
            "jaw_closing_axis": "x",
            "joint_names": [],
            "max_joint_speed": 1,
            "robot": None,
            "surface_guard": None,
        },
    }


class ScriptedHttpStation:
    def __init__(
        self, capture_observation: Callable[[], Awaitable[object]] | None = None
    ) -> None:
        self.url = ""
        self.streams: list[asyncio.StreamWriter] = []
        self.stream_arrived = asyncio.Condition()
        self.sequence = 0
        self.last_event_ids: list[str | None] = []
        self.lease_requested = asyncio.Event()
        self.grant_lease = asyncio.Event()
        self.lease_released = asyncio.Event()
        self.refuse_lease = False
        self.retained_run_records: list[dict[str, object]] = []
        self.capture_observation = capture_observation

    async def handle(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        try:
            header = await reader.readuntil(b"\r\n\r\n")
            lines = header.decode().split("\r\n")
            method, path, _ = lines[0].split(" ", 2)
            headers = {
                name.lower(): value
                for line in lines[1:]
                if ": " in line
                for name, value in [line.split(": ", 1)]
            }
            content_length = int(headers.get("content-length", "0"))
            if content_length:
                await reader.readexactly(content_length)
            if path == "/events":
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n"
                )
                await writer.drain()
                async with self.stream_arrived:
                    self.streams.append(writer)
                    self.last_event_ids.append(headers.get("last-event-id"))
                    self.stream_arrived.notify_all()
                if len(self.streams) == 1:
                    await self.event(
                        "snapshot",
                        {
                            "station_instance_id": STATION_ID,
                            "sequence": 0,
                            "run_sequence": 0,
                            "items": [state_message()],
                        },
                        writer,
                    )
                await reader.read()
                return
            if path == "/instance":
                result: object = {"id": STATION_ID}
            elif path == "/state":
                result = state_message(holder="agent")["state"]
            elif method == "POST" and path == "/observations":
                if self.capture_observation is not None:
                    result = await self.capture_observation()
                else:
                    result = {
                        "status": "observation_captured",
                        "state": state_message(holder="agent")["state"],
                        "cameras": {},
                        "missing_optional_cameras": [],
                    }
            elif path.startswith("/run/records?"):
                result = self.retained_run_records
            elif method == "POST" and path == "/leases":
                if self.refuse_lease:
                    body = b"The lease is held by 'x'."
                    writer.write(
                        b"HTTP/1.1 409 Conflict\r\nConnection: close\r\nContent-Length: "
                        + str(len(body)).encode()
                        + b"\r\n\r\n"
                        + body
                    )
                    await writer.drain()
                    return
                self.lease_requested.set()
                await self.grant_lease.wait()
                result = {
                    "lease_token": "token",
                    "lease_seconds": 15,
                    "status": {
                        "holder": "agent",
                        "label": "test",
                        "required_cameras": [],
                        "on_interruption": "wait_for_approval",
                        "expires_in_seconds": 15,
                    },
                }
            elif method == "POST" and path in ("/leases/renew", "/leases/release"):
                if path == "/leases/release":
                    self.lease_released.set()
                result = None
            else:
                raise AssertionError(f"Unexpected HTTP request {method} {path}")
            payload = json.dumps(result).encode() if result is not None else b""
            status = b"200 OK" if result is not None else b"204 No Content"
            writer.write(
                b"HTTP/1.1 "
                + status
                + b"\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: "
                + str(len(payload)).encode()
                + b"\r\n\r\n"
                + payload
            )
            await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()

    async def event(
        self,
        kind: str,
        data: dict[str, Any],
        writer: asyncio.StreamWriter | None = None,
    ) -> None:
        self.sequence += 1
        target = writer or self.streams[-1]
        target.write(
            f"id: {STATION_ID}:{self.sequence}\nevent: {kind}\ndata: {json.dumps(data)}\n\n".encode()
        )
        await target.drain()

    async def wait_for_stream(self, count: int) -> None:
        async with self.stream_arrived:
            await self.stream_arrived.wait_for(lambda: len(self.streams) >= count)

    async def drop_stream(self) -> None:
        self.streams[-1].close()
        await self.streams[-1].wait_closed()


@asynccontextmanager
async def scripted_http_station(
    *, capture_observation: Callable[[], Awaitable[object]] | None = None
) -> AsyncGenerator[ScriptedHttpStation]:
    station = ScriptedHttpStation(capture_observation)
    server = await asyncio.start_server(station.handle, "127.0.0.1", 0)
    station.url = f"http://127.0.0.1:{server.sockets[0].getsockname()[1]}"
    try:
        yield station
    finally:
        for stream in station.streams:
            stream.close()
        server.close()
        await server.wait_closed()
