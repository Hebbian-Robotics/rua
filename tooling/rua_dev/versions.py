import tomllib
from pathlib import Path


def check_release_versions(repository: Path) -> None:
    manifests = {
        "rua": ("pyproject.toml", "project"),
        "server": ("station/Cargo.toml", "package"),
        "server-core": ("server-core/Cargo.toml", "package"),
        "media": ("livekit-cameras/Cargo.toml", "package"),
    }
    sections = {}
    for name, (filename, section) in manifests.items():
        with (repository / filename).open("rb") as source:
            sections[name] = tomllib.load(source)[section]
    versions = {name: section["version"] for name, section in sections.items()}
    if len(set(versions.values())) != 1:
        raise SystemExit(f"Release package versions do not match: {versions}")
    print(f"RUA release packages agree on version {versions['rua']}")
