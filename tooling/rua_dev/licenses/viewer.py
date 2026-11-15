"""Copy viewer dependency licenses into a server-wheel staging directory."""

import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any


def collect_viewer_licenses(repository_root: Path, destination: Path) -> None:
    listing = subprocess.run(
        ["pnpm", "licenses", "list", "--prod", "--json"],
        cwd=repository_root,
        capture_output=True,
        text=True,
        check=True,
    )
    groups: dict[str, list[dict[str, Any]]] = json.loads(listing.stdout)
    destination.mkdir(parents=True, exist_ok=True)
    packages: list[dict[str, Any]] = []
    seen: set[tuple[str, str]] = set()
    for entries in groups.values():
        for entry in entries:
            for package_path in entry["paths"]:
                package_directory = Path(package_path)
                package_metadata = json.loads(
                    (package_directory / "package.json").read_text()
                )
                package_name = package_metadata["name"]
                version = package_metadata["version"]
                identity = (package_name, version)
                if identity in seen:
                    continue
                seen.add(identity)
                safe_name = re.sub(
                    r"[^a-zA-Z0-9._-]", "_", package_name.lstrip("@").replace("/", "-")
                )
                package_key = f"{safe_name}-{version}"
                target = destination / package_key
                target.mkdir()
                sources = [
                    path
                    for path in package_directory.iterdir()
                    if path.is_file()
                    and path.name.lower().startswith(
                        ("license", "licence", "copying", "notice")
                    )
                ]
                fallback = repository_root / "licenses" / "viewer" / package_key
                if fallback.is_dir():
                    sources.extend(
                        path for path in fallback.iterdir() if path.is_file()
                    )
                if not sources:
                    raise RuntimeError(
                        f"No license file for {package_name} {version}; add a verified upstream copy"
                    )
                for source in sources:
                    shutil.copy2(source, target / source.name)
                packages.append(
                    {
                        "name": package_name,
                        "version": version,
                        "license": entry["license"],
                        "directory": package_key,
                        "files": sorted(path.name for path in sources),
                    }
                )
    packages.sort(key=lambda package: (package["name"], package["version"]))
    (destination / "manifest.json").write_text(
        json.dumps(packages, indent=2, ensure_ascii=False) + "\n"
    )
