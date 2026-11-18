import os
import shutil
import sys
import sysconfig
from pathlib import Path


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
    os.execv(str(executable), ["rua", *sys.argv[1:]])
