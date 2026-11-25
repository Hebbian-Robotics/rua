import subprocess
from pathlib import Path

from pydantic import BaseModel


class CargoBuildDirectories(BaseModel):
    target_directory: Path


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
    target_directory = CargoBuildDirectories.model_validate_json(
        metadata.stdout
    ).target_directory
    return target_directory / "debug/hebbian-rua-server"
