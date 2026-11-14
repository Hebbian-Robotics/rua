"""Decode the station's HTTP event stream independently of its transport."""

import json
from collections.abc import AsyncIterator
from dataclasses import dataclass

import httpx2


@dataclass(frozen=True)
class StationStreamEvent:
    """An SSE event and its resumable station cursor, when present."""

    kind: str
    data: object
    event_id: str | None


async def decode_station_events(
    lines: AsyncIterator[str],
) -> AsyncIterator[StationStreamEvent]:
    """Decode SSE lines, preserving event IDs and ignoring keepalive comments."""
    event_kind = "message"
    event_id: str | None = None
    data_lines: list[str] = []
    async for line in lines:
        if not line:
            if data_lines:
                raw_data = "\n".join(data_lines)
                try:
                    data: object = json.loads(raw_data)
                except json.JSONDecodeError:
                    data = raw_data
                yield StationStreamEvent(event_kind, data, event_id)
            event_kind = "message"
            data_lines = []
            continue
        if line.startswith(":"):
            continue
        field, separator, value = line.partition(":")
        if separator and value.startswith(" "):
            value = value[1:]
        if field == "event":
            event_kind = value
        elif field == "id" and "\x00" not in value:
            event_id = value
        elif field == "data":
            data_lines.append(value)


async def stream_station_events(
    client: httpx2.AsyncClient,
) -> AsyncIterator[StationStreamEvent]:
    """Reconnect a station event stream, requesting a snapshot after replay gaps."""
    import asyncio

    cursor: str | None = None
    retry_delay = 0.1
    while True:
        headers = {"Last-Event-ID": cursor} if cursor is not None else {}
        try:
            async with client.stream(
                "GET", "/events", headers=headers, timeout=None
            ) as response:
                if response.status_code == 409 and cursor is not None:
                    cursor = None
                    yield StationStreamEvent(
                        "resync_required", "Event cursor expired", None
                    )
                    continue
                response.raise_for_status()
                retry_delay = 0.1
                async for event in decode_station_events(response.aiter_lines()):
                    if event.kind == "resync_required" or (
                        isinstance(event.data, dict)
                        and event.data.get("kind") == "resync_required"
                    ):
                        cursor = None
                        yield event
                        break
                    cursor = event.event_id or cursor
                    yield event
            yield StationStreamEvent("disconnected", "Event stream ended", None)
            await asyncio.sleep(retry_delay)
        except (httpx2.HTTPError, OSError):
            yield StationStreamEvent("disconnected", "Event stream unavailable", None)
            await asyncio.sleep(retry_delay)
            retry_delay = min(1.0, retry_delay * 2)
