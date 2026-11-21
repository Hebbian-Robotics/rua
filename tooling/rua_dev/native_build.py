import json
import subprocess
from pathlib import Path


def build_server() -> Path:
    subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--quiet",
            "--package",
            "hebbian-robot-use-server",
            "--bin",
            "hebbian-robot-use-server",
        ],
        check=True,
    )
    metadata = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True,
        text=True,
        check=True,
    )
    target_directory = Path(json.loads(metadata.stdout)["target_directory"])
    return target_directory / "debug/hebbian-robot-use-server"
