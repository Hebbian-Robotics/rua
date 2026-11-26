import os
import subprocess
import sys
from pathlib import Path


def test_so101_launcher_finds_executable_on_path(tmp_path: Path) -> None:
    executable = tmp_path / "so101"
    executable.write_text('#!/bin/sh\nprintf "%s\\n" "$SO101_SETUP_DIRECTORY" "$@"\n')
    executable.chmod(0o755)
    environment = os.environ | {
        "PATH": f"{tmp_path}:/usr/bin:/bin",
        "SO101_SETUP_DIRECTORY": str(tmp_path / "arm-configs"),
    }
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
        str(tmp_path / "arm-configs"),
        "setup",
        "--help",
    ]
