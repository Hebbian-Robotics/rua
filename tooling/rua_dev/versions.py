import json
import tomllib
from pathlib import Path


def check_release_versions(repository: Path, *, tag: str | None = None) -> None:
    manifests = {
        "rua": ("pyproject.toml", "project"),
        "server": ("station/Cargo.toml", "package"),
        "server-core": ("server-core/Cargo.toml", "package"),
        "media": ("livekit-cameras/Cargo.toml", "package"),
        "cli": ("cli/Cargo.toml", "package"),
        "python-bindings": ("python-bindings/Cargo.toml", "package"),
        "rust-sdk": ("sdk/Cargo.toml", "package"),
        "rust-contracts": ("contracts/Cargo.toml", "package"),
    }
    sections = {}
    for name, (filename, section) in manifests.items():
        with (repository / filename).open("rb") as source:
            sections[name] = (tomllib.load(source), section)
    versions = {
        name: manifest[section]["version"]
        for name, (manifest, section) in sections.items()
    }
    typescript_manifest = repository / "typescript-client/package.json"
    if typescript_manifest.is_file():
        versions["typescript-sdk"] = json.loads(typescript_manifest.read_text())[
            "version"
        ]
    if len(set(versions.values())) != 1:
        raise SystemExit(f"Release package versions do not match: {versions}")
    rust_sdk, _ = sections["rust-sdk"]
    contracts_version = rust_sdk["dependencies"]["hebbian-contracts"]["version"]
    if contracts_version != versions["rust-contracts"]:
        raise SystemExit(
            "The Rust SDK dependency must match the contracts crate version"
        )
    expected_tag = f"v{versions['rua']}"
    if tag is not None and tag != expected_tag:
        raise SystemExit(f"Release tag {tag} must equal {expected_tag}")
    print(f"RUA release packages agree on version {versions['rua']}")
