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
            "hebbian-rua-server",
            "--bin",
            "hebbian-rua-server",
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
    return target_directory / "debug/hebbian-rua-server"
