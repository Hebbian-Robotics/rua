import os
import subprocess
import sys
from pathlib import Path

import pytest


@pytest.mark.parametrize("custom_setup_directory", [False, True])
def test_so101_launcher_preserves_setup_directory_environment(
    tmp_path: Path, custom_setup_directory: bool
) -> None:
    executable = tmp_path / "so101"
    executable.write_text(
        '#!/bin/sh\nprintf "%s\\n" "${SO101_SETUP_DIRECTORY-unset}" "$@"\n'
    )
    executable.chmod(0o755)
    environment = os.environ | {
        "PATH": f"{tmp_path}:/usr/bin:/bin",
        "HEBBIAN_SO101_CONFIG_DIRECTORY": str(tmp_path / "old-arm-configs"),
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
    }
    environment.pop("SO101_SETUP_DIRECTORY", None)
    if custom_setup_directory:
        environment["SO101_SETUP_DIRECTORY"] = str(tmp_path / "arm-configs")
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import sysconfig; sysconfig.get_path = lambda name: '/nonexistent'; "
            "from hebbian_rua.cli import main; main()",
            "so101",
            "setup",
            "--help",
        ],
        env=environment,
        check=True,
        capture_output=True,
        text=True,
    )
    assert result.stdout.splitlines() == [
        str(tmp_path / "arm-configs") if custom_setup_directory else "unset",
        "setup",
        "--help",
    ]
