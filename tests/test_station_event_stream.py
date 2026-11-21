import asyncio
from collections.abc import Awaitable, Callable

import pytest

from hebbian_rua.client import station_connection
from hebbian_rua.client.station_connection import StationConnection

SNAPSHOT = b'id: 7\nevent: snapshot\ndata: {"items": [], "run_sequence": 0}\n\n'

type StreamBody = Callable[[asyncio.StreamWriter], Awaitable[None]]


class LocalStation:
    def __init__(self, first_stream: StreamBody, later_streams: StreamBody) -> None:
        self.event_requests: list[dict[str, str]] = []
        self._first_stream = first_stream
        self._later_streams = later_streams
        self._writers: set[asyncio.StreamWriter] = set()
        self._server: asyncio.Server | None = None

    async def __aenter__(self) -> str:
        self._server = await asyncio.start_server(self._serve, "127.0.0.1", 0)
        port = self._server.sockets[0].getsockname()[1]
        return f"http://127.0.0.1:{port}"

    async def __aexit__(self, *_: object) -> None:
        assert self._server is not None
        self._server.close()
        for writer in self._writers:
            writer.close()
        await self._server.wait_closed()

    async def _serve(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        self._writers.add(writer)
        request = await reader.readuntil(b"\r\n\r\n")
        request_line, *header_lines = request.decode().strip().split("\r\n")
        assert request_line.startswith("GET /events ")
        headers = {
            name.lower(): value.strip()
            for name, _, value in (line.partition(":") for line in header_lines)
        }
        self.event_requests.append(headers)
        writer.write(
            b"HTTP/1.1 200 OK\r\n"
            b"Content-Type: text/event-stream\r\n"
            b"Connection: close\r\n\r\n"
        )
        stream = (
            self._first_stream if len(self.event_requests) == 1 else self._later_streams
        )
        try:
            await stream(writer)
            await asyncio.Event().wait()
        except (asyncio.CancelledError, ConnectionError):
            pass
        finally:
            writer.close()


async def send_snapshot(writer: asyncio.StreamWriter) -> None:
    writer.write(SNAPSHOT)
    await writer.drain()


async def stay_silent(_: asyncio.StreamWriter) -> None:
    pass


async def wait_until_connected(connection: StationConnection) -> None:
    await connection.start()
    async with asyncio.timeout(2.0):
        await connection.wait_for_change(((), False))


def test_a_silent_stream_reconnects_and_fails_the_pending_command(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(station_connection, "EVENT_STREAM_READ_TIMEOUT_SECONDS", 0.2)
    monkeypatch.setattr(station_connection, "DISCONNECT_PATIENCE_SECONDS", 1.0)

    async def scenario() -> list[dict[str, str]]:
        station = LocalStation(send_snapshot, stay_silent)
        async with station as server:
            connection = StationConnection(server)
            try:
                await wait_until_connected(connection)
                pending = asyncio.create_task(connection.wait_for_outcome("command-1"))
                async with asyncio.timeout(5.0):
                    with pytest.raises(ConnectionError, match="unreachable"):
                        await pending
            finally:
                await connection.close()
        return station.event_requests

    event_requests = asyncio.run(scenario())

    assert len(event_requests) >= 2
    assert "last-event-id" not in event_requests[0]
    assert all(request["last-event-id"] == "7" for request in event_requests[1:])


def test_keepalive_comments_keep_an_idle_stream_connected(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(station_connection, "EVENT_STREAM_READ_TIMEOUT_SECONDS", 0.2)
    monkeypatch.setattr(station_connection, "DISCONNECT_PATIENCE_SECONDS", 0.5)

    async def send_snapshot_then_keepalives(writer: asyncio.StreamWriter) -> None:
        await send_snapshot(writer)
        while True:
            await asyncio.sleep(0.05)
            writer.write(b":\n\n")
            await writer.drain()

    async def scenario() -> tuple[bool, tuple[object, ...], int]:
        station = LocalStation(send_snapshot_then_keepalives, stay_silent)
        async with station as server:
            connection = StationConnection(server)
            try:
                await wait_until_connected(connection)
                pending = asyncio.create_task(connection.wait_for_outcome("command-1"))
                await asyncio.sleep(1.0)
                still_running = not pending.done()
                stamp = connection.change_stamp()
                pending.cancel()
            finally:
                await connection.close()
        return still_running, stamp, len(station.event_requests)

    still_running, stamp, event_request_count = asyncio.run(scenario())

    assert still_running
    assert stamp == ((), True)
    assert event_request_count == 1
