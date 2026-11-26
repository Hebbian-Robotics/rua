import os
import shutil
import sys
import sysconfig
from pathlib import Path


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "so101":
        executable = Path(sysconfig.get_path("scripts")) / "so101"
        if not executable.is_file():
            raise SystemExit(
                "SO101 CLI is missing. Install a RUA release wheel that includes it."
            )
        environment = os.environ.copy()
        if not environment.get("SO101_SETUP_DIRECTORY"):
            config_home = environment.get("XDG_CONFIG_HOME") or str(
                Path.home() / ".config"
            )
            environment["SO101_SETUP_DIRECTORY"] = environment.get(
                "HEBBIAN_SO101_CONFIG_DIRECTORY"
            ) or str(Path(config_home) / "rua/so101-arm-configs")
        os.execve(str(executable), ["so101", *sys.argv[2:]], environment)

    executable_name = "hebbian-rua-server"
    installed_executable = Path(sysconfig.get_path("scripts")) / executable_name
    override = os.environ.get("HEBBIAN_RUA_SERVER")
    if override:
        candidates = [Path(override)]
    else:
        checkout = Path(__file__).resolve().parent.parent
        on_path = shutil.which(executable_name)
        candidates = [
            installed_executable,
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
    environment = os.environ.copy()
    if not override and executable == installed_executable:
        # WHY: A symlink install can resolve the executable into a package cache.
        environment["HEBBIAN_RUA_INSTALL_DATA"] = sysconfig.get_path("data")
    os.execve(str(executable), ["rua", *sys.argv[1:]], environment)
