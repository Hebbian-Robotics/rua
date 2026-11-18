import json
import shutil
import subprocess
from pathlib import Path
from typing import Any


def collect_rust_licenses(
    repository_root: Path, package_name: str, destination: Path, target_triple: str
) -> None:
    metadata_process = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            target_triple,
        ],
        cwd=repository_root,
        capture_output=True,
        text=True,
        check=True,
    )
    metadata: dict[str, Any] = json.loads(metadata_process.stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    fallback_root = repository_root / "licenses" / "rust"
    fallback_entries = json.loads((fallback_root / "manifest.json").read_text())
    fallbacks = {(entry["name"], entry["version"]): entry for entry in fallback_entries}
    root_id = next(
        package["id"]
        for package in metadata["packages"]
        if package["name"] == package_name
        and package["id"] in metadata["workspace_members"]
    )
    pending = [root_id]
    included: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in included:
            continue
        included.add(package_id)
        pending.extend(
            dependency["pkg"]
            for dependency in nodes[package_id]["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        )

    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(fallback_root / "README.md", destination / "README.md")
    manifest: list[dict[str, Any]] = []
    for package_id in included:
        package = packages[package_id]
        package_key = f"{package['name']}-{package['version']}"
        package_directory = Path(package["manifest_path"]).parent
        license_files = [
            path
            for path in package_directory.iterdir()
            if path.is_file()
            and path.name.lower().startswith(
                ("license", "licence", "copying", "notice")
            )
        ]
        declared_license_file = package.get("license_file")
        if declared_license_file:
            license_path = package_directory / declared_license_file
            if license_path.is_file() and license_path not in license_files:
                license_files.append(license_path)
        if not license_files and package_id in metadata["workspace_members"]:
            license_files.append(repository_root / "LICENSE")
        fallback = fallbacks.get((package["name"], package["version"]))
        if fallback:
            license_files.extend(
                fallback_root / fallback["directory"] / filename
                for filename in fallback["files"]
            )
        if license_files:
            package_target = destination / package_key
            package_target.mkdir(exist_ok=True)
            for source in license_files:
                shutil.copy2(source, package_target / source.name)
        manifest.append(
            {
                "name": package["name"],
                "version": package["version"],
                "license": package.get("license"),
                "directory": package_key,
                "files": sorted({path.name for path in license_files}),
                "upstream": None
                if not fallback
                else {
                    "repository": fallback["repository"],
                    "commit": fallback["commit"],
                    **(
                        {"license_text_source": fallback["license_text_source"]}
                        if "license_text_source" in fallback
                        else {}
                    ),
                },
            }
        )
    manifest.sort(key=lambda package: (package["name"], package["version"]))
    (destination / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"
    )
    missing = [
        f"{package['name']} {package['version']}"
        for package in manifest
        if not package["files"]
    ]
    if missing:
        raise ValueError("Missing Rust dependency license files: " + ", ".join(missing))
