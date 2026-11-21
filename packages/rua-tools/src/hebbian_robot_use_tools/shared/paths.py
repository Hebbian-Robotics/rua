import sys
from pathlib import Path

REPOSITORY_ROOT = next(
    (
        directory
        for directory in Path(__file__).resolve().parents
        if (directory / "Cargo.toml").is_file() and (directory / "schemas").is_dir()
    ),
    Path(sys.prefix) / "share/hebbian-robot-use",
)
