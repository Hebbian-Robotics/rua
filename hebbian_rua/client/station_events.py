"""Consume the station's resumable HTTP event stream."""

import json
from collections.abc import AsyncGenerator
from contextlib import aclosing
from dataclasses import dataclass

import httpx2
from tenacity import AsyncRetrying, retry_if_exception_type, stop_never
from tenacity.wait import wait_exponential_jitter


@dataclass(frozen=True)
class StationStreamEvent:
    """An SSE event and its resumable station cursor, when present."""

    kind: str
    data: object
    event_id: str | None


class _StationStreamUnavailable(Exception):
    """An attempt ended before it delivered a usable station event."""


async def _read_stream(
    client: httpx2.AsyncClient,
    cursor: str | None,
    read_timeout_seconds: float,
) -> AsyncGenerator[StationStreamEvent]:
    headers = {"Last-Event-ID": cursor} if cursor is not None else {}
    # A snapshot can include retained logs for every arm.
    async with client.sse(
        "/events",
        headers=headers,
        timeout=httpx2.Timeout(None, read=read_timeout_seconds),
        max_event_size=None,
    ) as source:
        response = source.response
        if response.status_code == 409 and cursor is not None:
            yield StationStreamEvent(
                "resync_required", "Station event cursor expired", None
            )
            return
        response.raise_for_status()
        async for message in source:
            # HTTPX2 also yields metadata-only blocks with empty data.
            if not message.data:
                continue
            try:
                data: object = message.json()
            except json.JSONDecodeError:
                data = message.data
            yield StationStreamEvent(message.event, data, message.id or None)


async def stream_station_events(
    client: httpx2.AsyncClient,
    *,
    read_timeout_seconds: float,
) -> AsyncGenerator[StationStreamEvent]:
    """Reconnect on read inactivity, requesting a snapshot after replay gaps."""
    cursor: str | None = None

    async def session() -> AsyncGenerator[StationStreamEvent]:
        nonlocal cursor
        received_station_event = False
        while True:
            resnapshot = False
            detail = "Event stream ended"
            try:
                async with aclosing(
                    _read_stream(client, cursor, read_timeout_seconds)
                ) as events:
                    async for event in events:
                        if event.kind == "resync_required" or (
                            isinstance(event.data, dict)
                            and event.data.get("kind") == "resync_required"
                        ):
                            resnapshot = cursor is not None
                            cursor = None
                            yield event
                            break
                        cursor = event.event_id or cursor
                        yield event
                        if event.kind in {"snapshot", "station"}:
                            received_station_event = True
            except (httpx2.HTTPError, OSError):
                detail = "Event stream unavailable"
            if resnapshot and not received_station_event:
                continue
            yield StationStreamEvent("disconnected", detail, None)
            if not received_station_event:
                raise _StationStreamUnavailable(detail)
            return

    # A session that delivered an event restarts with a fresh retry budget.
    while True:
        async for attempt in AsyncRetrying(
            retry=retry_if_exception_type(_StationStreamUnavailable),
            stop=stop_never,
            wait=wait_exponential_jitter(initial=0.1, max=1.0, jitter=0.1),
        ):
            with attempt:
                async with aclosing(session()) as events:
                    async for event in events:
                        yield event
