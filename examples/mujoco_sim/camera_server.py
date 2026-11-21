# CONSTRAINT: OpenGL contexts belong to the simulator thread. Socket requests queue until
# serve_pending renders after a physics step; the socket thread never renders.

import json
import socket
import threading
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any

FRAME_TIMEOUT_SECONDS = 5.0
CAMERA_SOCKET_NAME = "scene.camera.sock"


class _Request:
    def __init__(self, camera: str) -> None:
        self.camera = camera
        self.done = threading.Event()
        self.jpeg = b""
        self.error: str | None = None


class CameraServer:
    def __init__(
        self, path: Path, names: Sequence[str], render: Callable[[str], bytes]
    ) -> None:
        self.names = list(names)
        self._render = render
        self._pending: list[_Request] = []
        self._lock = threading.Lock()
        path.unlink(missing_ok=True)
        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._listener.bind(str(path))
        self._listener.listen()
        threading.Thread(target=self._accept, daemon=True).start()

    def serve_pending(self) -> None:
        # WHY: Render at most one pending frame per physics step: software rendering can take
        # hundreds of milliseconds, so draining the queue would starve control replies.
        with self._lock:
            if not self._pending:
                return
            request = self._pending.pop(0)
        try:
            request.jpeg = self._render(request.camera)
        except Exception as error:  # reported to the requester, not fatal
            request.error = str(error)
        request.done.set()

    def close(self) -> None:
        self._listener.close()

    def _accept(self) -> None:
        while True:
            try:
                connection, _ = self._listener.accept()
            except OSError:
                return
            threading.Thread(
                target=self._serve, args=(connection,), daemon=True
            ).start()

    def _serve(self, connection: socket.socket) -> None:
        with connection, connection.makefile("rwb") as stream:
            for line in stream:
                request = json.loads(line)
                if request.get("list"):
                    reply: dict[str, Any] = {"names": self.names}
                    stream.write(json.dumps(reply).encode() + b"\n")
                    stream.flush()
                    continue
                camera = str(request.get("camera"))
                if camera not in self.names:
                    reply = {"error": f"No camera {camera}; choose from {self.names}"}
                    stream.write(json.dumps(reply).encode() + b"\n")
                    stream.flush()
                    continue
                pending = _Request(camera)
                with self._lock:
                    self._pending.append(pending)
                if not pending.done.wait(FRAME_TIMEOUT_SECONDS):
                    reply = {"error": "The simulation did not render a frame"}
                elif pending.error is not None:
                    reply = {"error": pending.error}
                else:
                    reply = {"bytes": len(pending.jpeg)}
                stream.write(json.dumps(reply).encode() + b"\n")
                if "bytes" in reply:
                    stream.write(pending.jpeg)
                stream.flush()
