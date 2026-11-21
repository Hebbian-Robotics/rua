import os
import shutil
from pathlib import Path

from hebbian_robot_use_tools.shared.paths import REPOSITORY_ROOT

SERVER_ENVIRONMENT_VARIABLE = "HEBBIAN_ROBOT_USE_SERVER"


def find_binary(name: str, environment_variable: str) -> Path | None:
    override = os.environ.get(environment_variable)
    if override:
        return Path(override)
    on_path = shutil.which(name)
    # WHY: Prefer the development build to an older release build during development.
    candidates = [
        REPOSITORY_ROOT / "target" / "debug" / name,
        *([Path(on_path)] if on_path else []),
        REPOSITORY_ROOT / "target" / "release" / name,
    ]
    return next((path for path in candidates if path.is_file()), None)


def server_binary() -> Path:
    found = find_binary("hebbian-robot-use-server", SERVER_ENVIRONMENT_VARIABLE)
    if found is None:
        raise FileNotFoundError(
            "No station binary: install the `hebbian-rua` wheel, run "
            f"`cargo build`, or set {SERVER_ENVIRONMENT_VARIABLE}"
        )
    return found
