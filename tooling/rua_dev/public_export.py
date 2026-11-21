import hashlib
import json
import shutil
import subprocess
import tomllib
from pathlib import Path

import tomli_w
from packaging.requirements import Requirement
from pydantic import BaseModel, ConfigDict


class ExportManifest(BaseModel):
    model_config = ConfigDict(extra="forbid")

    paths: list[str]
    exclude: list[str]
    workspace_members: list[str]
    symlink_targets: list[str] = []
    python_excluded_dependencies: list[str] = []
    python_excluded_groups: list[str] = []


def prepare_python_workspace(destination: Path, manifest: ExportManifest) -> None:
    path = destination / "pyproject.toml"
    if not path.is_file():
        return
    project = tomllib.loads(path.read_text())
    excluded = set(manifest.python_excluded_dependencies)
    groups = project.get("dependency-groups", {})
    for group in manifest.python_excluded_groups:
        groups.pop(group, None)
    for group, dependencies in groups.items():
        groups[group] = [
            dependency
            for dependency in dependencies
            if not isinstance(dependency, str)
            or Requirement(dependency).name not in excluded
        ]
    tools = project.get("tool", {})
    uv = tools.get("uv", {})
    workspace = uv.get("workspace", {})
    if "members" in workspace:
        workspace["members"] = [
            member
            for member in workspace["members"]
            if (destination / member / "pyproject.toml").is_file()
        ]
    sources = uv.get("sources", {})
    for dependency in excluded:
        sources.pop(dependency, None)
    if "import-linter" in excluded:
        tools.pop("importlinter", None)
    # CONSTRAINT: The exported tests do not use the development workspace's test lanes.
    tools.get("pytest", {}).get("ini_options", {}).pop("markers", None)
    typing_environment = tools.get("ty", {}).get("environment", {})
    if "extra-paths" in typing_environment:
        typing_environment["extra-paths"] = [
            directory
            for directory in typing_environment["extra-paths"]
            if (destination / directory).is_dir()
        ]
    path.write_text(tomli_w.dumps(project))


def export_public(repository_directory: Path, destination: Path) -> None:
    repository_directory = repository_directory.resolve()
    destination = destination.resolve()
    if destination.is_relative_to(repository_directory):
        raise ValueError(
            "Export outside the source checkout to avoid recursive exports"
        )
    # CONSTRAINT: Export never modifies an existing checkout or directory.
    if destination.exists():
        raise ValueError(
            f"Destination already exists: {destination}; choose a new directory"
        )
    template_directory = repository_directory / "tooling/public_export"
    manifest = ExportManifest.model_validate(
        tomllib.loads((template_directory / "manifest.toml").read_text())
    )
    tracked_paths = (
        subprocess.check_output(["git", "ls-files", "-z"], cwd=repository_directory)
        .decode()
        .split("\0")
    )
    selected_paths = sorted(
        relative_path
        for relative_path in tracked_paths
        if relative_path
        and any(
            relative_path == allowed_path
            or relative_path.startswith(f"{allowed_path}/")
            for allowed_path in manifest.paths
        )
        and relative_path not in manifest.exclude
    )
    for relative_path in selected_paths:
        source = repository_directory / relative_path
        resolved_source = source.resolve()
        if not source.is_file() or not resolved_source.is_relative_to(
            repository_directory
        ):
            raise ValueError(f"Export requires a regular file: {relative_path}")
        resolved_relative_path = resolved_source.relative_to(
            repository_directory
        ).as_posix()
        allowed_shared_asset = resolved_relative_path in tracked_paths and any(
            resolved_relative_path == allowed_target
            or resolved_relative_path.startswith(f"{allowed_target}/")
            for allowed_target in manifest.symlink_targets
        )
        if resolved_relative_path not in selected_paths and not allowed_shared_asset:
            raise ValueError(f"Export requires a regular file: {relative_path}")
    destination.mkdir(parents=True)
    for relative_path in selected_paths:
        output_path = destination / relative_path
        output_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(repository_directory / relative_path, output_path)
    cargo_manifest_path = destination / "Cargo.toml"
    cargo_manifest = cargo_manifest_path.read_text()
    cargo_manifest_lines = [
        f"members = {json.dumps(manifest.workspace_members)}"
        if line.startswith("members = ")
        else line
        for line in cargo_manifest.splitlines()
        if not line.startswith("default-members = ")
    ]
    # WHY: Keep media opt-in: its native prerequisites are unnecessary for robot control.
    member_index = next(
        index
        for index, line in enumerate(cargo_manifest_lines)
        if line.startswith("members = ")
    )
    cargo_manifest_lines.insert(
        member_index + 1,
        f"default-members = {json.dumps([member for member in manifest.workspace_members if member != 'livekit-cameras'])}",
    )
    cargo_manifest_path.write_text("\n".join(cargo_manifest_lines) + "\n")
    template_prefix = "tooling/public_export/files/"
    for relative_path in sorted(tracked_paths):
        if relative_path.startswith(template_prefix):
            template_path = repository_directory / relative_path
            if template_path.is_symlink() or not template_path.is_file():
                raise ValueError(f"Export requires a regular template: {relative_path}")
            output_path = destination / relative_path.removeprefix(template_prefix)
            output_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(template_path, output_path)
    prepare_python_workspace(destination, manifest)
    # WHY: Resolve the reduced workspace without compiling it, retaining pinned versions.
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=destination,
        stdout=subprocess.DEVNULL,
        check=True,
    )
    for project_directory in (".", "examples/python", "examples/mujoco_sim"):
        if (destination / project_directory / "pyproject.toml").is_file():
            subprocess.run(
                ["uv", "lock", "--project", project_directory],
                cwd=destination,
                check=True,
            )
    source_revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=repository_directory, text=True
    ).strip()
    (destination / "EXPORT.json").write_text(
        json.dumps(
            {
                "source_revision": source_revision,
                "source_dirty": bool(
                    subprocess.check_output(
                        ["git", "status", "--porcelain"], cwd=repository_directory
                    )
                ),
                "files": {
                    output_path.relative_to(destination).as_posix(): hashlib.sha256(
                        output_path.read_bytes()
                    ).hexdigest()
                    for output_path in sorted(destination.rglob("*"))
                    if output_path.is_file()
                },
            },
            indent=2,
        )
        + "\n"
    )
    print(f"Exported {len(selected_paths)} tracked files to {destination}")
    print("Local snapshot only. Follow CONTRIBUTING.md inside it to validate.")
