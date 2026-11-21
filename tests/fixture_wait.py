import asyncio
import time
from collections.abc import Awaitable, Callable


async def wait_until_async(
    condition: Callable[[], Awaitable[bool]],
    *,
    timeout: float = 5.0,
    interval: float = 0.02,
    message: str = "timed out",
) -> None:
    deadline = time.monotonic() + timeout
    while not await condition():
        assert time.monotonic() < deadline, message
        await asyncio.sleep(interval)


def wait_until(
    condition: Callable[[], bool],
    *,
    timeout: float = 10.0,
    interval: float = 0.02,
    message: str = "timed out",
) -> None:
    deadline = time.monotonic() + timeout
    while not condition():
        assert time.monotonic() < deadline, message
        time.sleep(interval)


def wait_for_value[Value](
    read: Callable[[], Value | None],
    *,
    timeout: float = 10.0,
    interval: float = 0.02,
    message: str = "timed out",
) -> Value:
    found: list[Value] = []

    def has_value() -> bool:
        value = read()
        if value is not None:
            found.append(value)
        return value is not None

    wait_until(has_value, timeout=timeout, interval=interval, message=message)
    return found[0]
