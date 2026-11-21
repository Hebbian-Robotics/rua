import asyncio
from contextlib import aclosing

import httpx2

from hebbian_rua.client.station_events import stream_station_events


def test_station_stream_resumes_after_the_last_payload_and_resets_on_conflict() -> None:
    async def exercise() -> None:
        responses = iter(
            (
                httpx2.Response(
                    200,
                    headers={"Content-Type": "text/event-stream"},
                    content=(
                        b": keepalive\n"
                        b"id: station:7\n\n"
                        b"event: station\n"
                        b'data: {"value":\n'
                        b"data: 2}\n\n"
                        b"id: station:99\n\n"
                    ),
                ),
                httpx2.Response(409),
                httpx2.Response(
                    200,
                    headers={"Content-Type": "text/event-stream"},
                    content=b"event: resync_required\ndata: fell behind\n\n",
                ),
            )
        )
        resume_cursors: list[str | None] = []

        def respond(request: httpx2.Request) -> httpx2.Response:
            resume_cursors.append(request.headers.get("Last-Event-ID"))
            return next(responses)

        async with (
            httpx2.AsyncClient(
                base_url="http://station.test", transport=httpx2.MockTransport(respond)
            ) as client,
            aclosing(stream_station_events(client, read_timeout_seconds=1.0)) as events,
        ):
            update = await anext(events)
            assert (update.kind, update.data) == ("station", {"value": 2})
            assert (await anext(events)).kind == "disconnected"
            expired_cursor = await anext(events)
            assert expired_cursor.kind == "resync_required"
            stream_notice = await anext(events)
            assert (stream_notice.kind, stream_notice.data) == (
                "resync_required",
                "fell behind",
            )
            assert resume_cursors == [None, "station:7", None]

    asyncio.run(exercise())
