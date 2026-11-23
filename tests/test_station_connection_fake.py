import asyncio
from collections.abc import Awaitable, Callable
from contextlib import aclosing

import pytest

from hebbian_rua.client.contract.station_api import RunEventRecord
from hebbian_rua.client.station_client import LeaseHeldElsewhere, StationClient
from tests.fixture_station_http import (
    STATION_ID,
    command_record,
    record_frame,
    scripted_http_station,
    state_message,
)

TEST_DEADLINE_SECONDS = 10


def run(test: Callable[[], Awaitable[None]]) -> None:
    asyncio.run(asyncio.wait_for(test(), timeout=TEST_DEADLINE_SECONDS))


def test_http_stream_resumes_and_command_outcome_waits_for_state() -> None:
    async def exercise() -> None:
        async with (
            scripted_http_station() as station,
            StationClient(station.url) as client,
        ):
            await client.connect()
            await station.event(
                "station", record_frame("follower", 1, command_record("c"))
            )
            await station.event("station", state_message(sampled_at=1))
            await client.wait_for_state(
                lambda state: (
                    state.arms[0].sampled_unix_nano is not None
                    and state.arms[0].sampled_unix_nano.root == "1000000000"
                )
            )
            pending = asyncio.create_task(client.wait_for_command("c"))
            await station.drop_stream()
            await station.wait_for_stream(2)
            assert station.last_event_ids[-1] == f"{STATION_ID}:3"
            await station.event(
                "station",
                record_frame("follower", 2, command_record("c", "5000000000")),
            )
            await station.event("station", state_message(sampled_at=2, active="c"))
            await client.wait_for_state(
                lambda state: (
                    state.arms[0].sampled_unix_nano is not None
                    and state.arms[0].sampled_unix_nano.root == "2000000000"
                )
            )
            assert not pending.done()
            await station.event("station", state_message(sampled_at=3, last="c"))
            assert (await pending).command_id == "c"

    run(exercise)


def test_overlapping_acquisitions_are_rejected_and_disconnect_loses_the_lease() -> None:
    async def exercise() -> None:
        async with (
            scripted_http_station() as station,
            StationClient(station.url) as client,
        ):
            acquired = asyncio.Event()

            async def acquire() -> None:
                async with client.holding_lease("test"):
                    acquired.set()
                    await client.lease_lost.wait()

            first = asyncio.create_task(acquire())
            await station.lease_requested.wait()
            with pytest.raises(ConnectionError, match="acquiring"):
                async with client.holding_lease("other"):
                    pass
            station.grant_lease.set()
            await acquired.wait()
            with pytest.raises(ConnectionError, match="acquiring"):
                async with client.holding_lease("while-held"):
                    pytest.fail("a held lease cannot be acquired again")
            await station.drop_stream()
            await first
            assert station.lease_released.is_set()

    run(exercise)


def test_lease_granted_after_stream_disconnect_is_released() -> None:
    async def exercise() -> None:
        async with (
            scripted_http_station() as station,
            StationClient(station.url) as client,
        ):

            async def acquire() -> None:
                async with client.holding_lease("test"):
                    pytest.fail(
                        "a lease cannot be handed out after losing its event stream"
                    )

            acquiring = asyncio.create_task(acquire())
            await station.lease_requested.wait()
            await station.drop_stream()
            await station.wait_for_stream(2)
            station.grant_lease.set()
            with pytest.raises(ConnectionError, match="lost while acquiring"):
                await acquiring
            assert station.lease_released.is_set()

    run(exercise)


def test_refused_lease_reports_the_current_holder_even_if_stream_is_stale() -> None:
    async def exercise() -> None:
        async with (
            scripted_http_station() as station,
            StationClient(station.url) as client,
        ):
            station.refuse_lease = True
            with pytest.raises(LeaseHeldElsewhere, match="agent 'x'"):
                async with client.holding_lease("test"):
                    pytest.fail("the station refused the lease")

    run(exercise)


def test_resumed_telemetry_delivers_retained_records_after_connect() -> None:
    async def exercise() -> None:
        async with (
            scripted_http_station() as station,
            StationClient(station.url, run_after=0) as client,
        ):
            expected_sequences = list(range(1, 4097))
            station.retained_run_records = [
                {
                    "type": "run_event",
                    "seq": sequence,
                    "received_unix_nano": "1000000000",
                    "event": {"type": "example"},
                }
                for sequence in expected_sequences
            ]
            await client.connect()
            received_sequences: list[int] = []
            async with aclosing(client.run_telemetry()) as telemetry:
                for _ in expected_sequences:
                    record = await anext(telemetry)
                    assert isinstance(record, RunEventRecord)
                    received_sequences.append(record.seq)
            assert received_sequences == expected_sequences

    run(exercise)
