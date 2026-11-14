"""Launch the RUA station; simulators are separate processes."""

import os
import shutil
import subprocess
import sys
import sysconfig
from pathlib import Path

from hebbian_rua.live_view import media_url_from_serve_arguments


def main() -> None:
    executable_name = "hebbian-robot-use-server"
    override = os.environ.get("HEBBIAN_ROBOT_USE_SERVER")
    if override:
        candidates = [Path(override)]
    else:
        checkout = Path(__file__).resolve().parent.parent
        on_path = shutil.which(executable_name)
        candidates = [
            Path(sysconfig.get_path("scripts")) / executable_name,
            checkout / "target/debug" / executable_name,
            *([Path(on_path)] if on_path else []),
            checkout / "target/release" / executable_name,
        ]
    executable = next(
        (candidate for candidate in candidates if candidate.is_file()), None
    )
    if executable is None:
        raise SystemExit(
            "RUA server binary is missing. Install a hebbian-rua wheel for your platform, or build the server from source."
        )
    arguments = sys.argv[1:]
    if arguments[:1] == ["serve"]:
        if "--no-view" in arguments:
            arguments.remove("--no-view")
        else:
            start_live_view(media_url_from_serve_arguments(arguments))
    os.execv(str(executable), ["rua", *arguments])


def start_live_view(media_url: str) -> None:
    """Open the camera window from a separate process, since exec replaces this one."""
    subprocess.Popen(
        [sys.executable, "-m", "hebbian_rua.live_view", media_url],
        stdin=subprocess.DEVNULL,
        start_new_session=True,  # Ctrl-C on the station leaves the page opener alone
    )
