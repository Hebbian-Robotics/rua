"""Consume the station's resumable HTTP event stream."""

import asyncio
import json
from collections.abc import AsyncGenerator
from dataclasses import dataclass

import httpx2


@dataclass(frozen=True)
class StationStreamEvent:
    """An SSE event and its resumable station cursor, when present."""

    kind: str
    data: object
    event_id: str | None


async def stream_station_events(
    client: httpx2.AsyncClient,
    *,
    read_timeout_seconds: float,
) -> AsyncGenerator[StationStreamEvent]:
    """Reconnect on read inactivity, requesting a snapshot after replay gaps."""
    cursor: str | None = None
    retry_delay = 0.1
    while True:
        headers = {"Last-Event-ID": cursor} if cursor is not None else {}
        try:
            # A snapshot can include retained logs for every arm.
            async with client.sse(
                "/events",
                headers=headers,
                timeout=httpx2.Timeout(None, read=read_timeout_seconds),
                max_event_size=None,
            ) as source:
                response = source.response
                if response.status_code == 409 and cursor is not None:
                    cursor = None
                    yield StationStreamEvent(
                        "resync_required", "Station event cursor expired", None
                    )
                    continue
                response.raise_for_status()
                retry_delay = 0.1
                async for message in source:
                    # HTTPX2 also yields metadata-only blocks with empty data.
                    if not message.data:
                        continue
                    try:
                        data: object = message.json()
                    except json.JSONDecodeError:
                        data = message.data
                    event = StationStreamEvent(message.event, data, message.id or None)
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
