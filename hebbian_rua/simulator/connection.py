"""Socket transport for a simulator implementing one station arm."""

import socket
import time
from pathlib import Path
from types import TracebackType

from hebbian_rua.simulator.contract import SimHello, SimState, SimStep

PROTOCOL_VERSION = 1
SOCKET_POLL_SECONDS = 0.05


class PlantDisconnected(ConnectionError):
    """The station closed its plant connection."""


class PlantConnection:
    """Exchange plant messages with one station arm over a Unix socket."""

    def __init__(self, stream: socket.socket) -> None:
        self._stream = stream
        self._lines = stream.makefile("rwb")

    @classmethod
    def connect(
        cls, socket_directory: Path, arm: str, *, timeout_seconds: float = 60.0
    ) -> "PlantConnection":
        """Connect to an arm's plant socket, waiting for the station to listen."""
        if not arm or Path(arm).name != arm or arm in {".", ".."}:
            raise ValueError("Arm must be a single nonempty socket name")
        path = socket_directory / f"{arm}.plant.sock"
        deadline = time.monotonic() + timeout_seconds
        while True:
            stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                stream.connect(str(path))
                return cls(stream)
            except OSError as error:
                stream.close()
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"No station is listening at {path}") from error
                time.sleep(
                    min(SOCKET_POLL_SECONDS, max(0, deadline - time.monotonic()))
                )

    def send_hello(self, joint_names: list[str], state: SimState) -> None:
        """Send the initial joint order and measured state."""
        self._write(
            SimHello(
                protocol_version=PROTOCOL_VERSION, joint_names=joint_names, state=state
            )
        )

    def read_step(self) -> SimStep:
        """Read the next target and simulation duration from the station."""
        line = self._lines.readline()
        if not line:
            raise PlantDisconnected("The station closed the plant connection")
        return SimStep.model_validate_json(line)

    def send_state(self, state: SimState) -> None:
        """Send measured state after advancing the simulator."""
        self._write(state)

    def _write(self, message: SimHello | SimState) -> None:
        self._lines.write(message.model_dump_json().encode() + b"\n")
        self._lines.flush()

    def close(self) -> None:
        """Close the connection and its socket."""
        try:
            self._lines.close()
        finally:
            self._stream.close()

    def __enter__(self) -> "PlantConnection":
        return self

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()
