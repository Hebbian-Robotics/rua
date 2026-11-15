"""Ensure the packages in one RUA release agree on their version."""

import tomllib
from pathlib import Path


def check_release_versions(repository: Path) -> None:
    manifests = {
        "rua": ("pyproject.toml", "project"),
        "server": ("station/Cargo.toml", "package"),
        "executor": ("executor/Cargo.toml", "package"),
        "media": ("livekit-cameras/Cargo.toml", "package"),
    }
    if (repository / "packages/rua-tools/pyproject.toml").is_file():
        manifests["tools"] = ("packages/rua-tools/pyproject.toml", "project")
    sections = {}
    for name, (filename, section) in manifests.items():
        with (repository / filename).open("rb") as source:
            sections[name] = tomllib.load(source)[section]
    versions = {name: section["version"] for name, section in sections.items()}
    if len(set(versions.values())) != 1:
        raise SystemExit(f"Release package versions do not match: {versions}")
    rua_requirement = f"hebbian-rua=={versions['rua']}"
    if "tools" in sections and rua_requirement not in sections["tools"]["dependencies"]:
        raise SystemExit(f"tools must depend on {rua_requirement}")
    print(f"RUA release packages agree on version {versions['rua']}")
