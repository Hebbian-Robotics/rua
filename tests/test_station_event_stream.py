import asyncio
from collections.abc import Awaitable, Callable

from hebbian_rua.client.station_client import StationClient

SNAPSHOT = b'id: 7\nevent: snapshot\ndata: {"station_instance_id":"station","sequence":7,"items":[],"run_sequence":0}\n\n'

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


def test_silent_stream_reconnects_with_last_received_event_id() -> None:
    async def exercise() -> None:
        station = LocalStation(send_snapshot, stay_silent)
        async with station as server, StationClient(server) as client:
            await client.connect()
            async with asyncio.timeout(40):
                while len(station.event_requests) < 2:
                    await asyncio.sleep(0.1)
            assert "last-event-id" not in station.event_requests[0]
            assert station.event_requests[1]["last-event-id"] == "7"

    asyncio.run(exercise())


def test_keepalive_comments_keep_an_idle_stream_connected() -> None:
    async def keepalives(writer: asyncio.StreamWriter) -> None:
        await send_snapshot(writer)
        while True:
            await asyncio.sleep(5)
            writer.write(b": keepalive\n\n")
            await writer.drain()

    async def exercise() -> None:
        station = LocalStation(keepalives, stay_silent)
        async with station as server, StationClient(server) as client:
            await client.connect()
            await asyncio.sleep(32)
            await client.connect()
            assert len(station.event_requests) == 1

    asyncio.run(exercise())
