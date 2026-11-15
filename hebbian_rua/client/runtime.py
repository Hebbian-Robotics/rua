"""A private per-user directory for what separate processes share."""

import os
import tempfile
from pathlib import Path

PRIVATE_DIRECTORY_MODE = 0o700


def runtime_directory() -> Path:
    """A directory only this user can use, for locks, logs and settings that
    separate processes share.

    It is under XDG_RUNTIME_DIR when the system provides one (already private to
    the user), else under the system temp directory with the user's id in its name.
    A directory someone else already made there is refused, not used: a shared
    /tmp lets another user pre-create a path and read or race what we put in it.
    """
    runtime_root = os.environ.get("XDG_RUNTIME_DIR")
    if runtime_root:
        directory = Path(runtime_root) / "hebbian-robot-use"
    else:
        directory = Path(tempfile.gettempdir()) / f"hebbian-robot-use-{os.getuid()}"
    directory.mkdir(mode=PRIVATE_DIRECTORY_MODE, parents=True, exist_ok=True)
    status = directory.stat()
    if status.st_uid != os.getuid():
        raise PermissionError(f"{directory} belongs to another user; not using it")
    if status.st_mode & 0o077:
        directory.chmod(PRIVATE_DIRECTORY_MODE)
    return directory
