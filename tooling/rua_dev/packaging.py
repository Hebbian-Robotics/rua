"""Stage redistributable assets and build server wheels."""

import os
import platform
import shutil
import subprocess
import tempfile
from collections.abc import Generator
from contextlib import ExitStack, contextmanager
from pathlib import Path

from rua_dev.licenses.rust import collect_rust_licenses
from rua_dev.licenses.viewer import collect_viewer_licenses
from rua_dev.licenses.webrtc import collect_webrtc_license
from rua_dev.models import BuildPlatform
from rua_dev.robot_assets import prepare_robot_assets
from rua_dev.versions import check_release_versions

MANYLINUX_IMAGE = "ghcr.io/pyo3/maturin@sha256:50bf2ff99eeff7b09332bad9baf4a5e925e2502304ae2f88f48ce8c06fd9957c"


def tracked_files(
    repository_directory: Path, *paths: str, include_untracked: bool = False
) -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "-z",
            *(
                ["--cached", "--others", "--exclude-standard"]
                if include_untracked
                else []
            ),
            "--",
            *paths,
        ],
        cwd=repository_directory,
        capture_output=True,
        check=True,
    )
    return [
        Path(os.fsdecode(filename))
        for filename in result.stdout.split(b"\0")
        if filename
    ]


def host_target() -> str:
    result = subprocess.run(
        ["rustc", "-vV"], capture_output=True, text=True, check=True
    )
    return next(
        line.removeprefix("host: ")
        for line in result.stdout.splitlines()
        if line.startswith("host: ")
    )


def stage_server_assets(
    repository_directory: Path, staging_directory: Path, *, include_viewer: bool
) -> None:
    robot_assets = prepare_robot_assets(repository_directory)
    asset_directory = staging_directory / "data/share/hebbian-robot-use"
    for directory in ("profiles", "robots"):
        (asset_directory / directory).mkdir(parents=True, exist_ok=True)
    for asset in tracked_files(repository_directory, "profiles", "robots"):
        if asset.is_relative_to("profiles/local") or asset.suffix in {
            ".onnx",
            ".pt",
            ".pth",
            ".safetensors",
        }:
            continue
        destination = asset_directory / asset
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(repository_directory / asset, destination)
    for source in robot_assets:
        destination = asset_directory / source.relative_to(repository_directory)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    for filename in ("LICENSE", "THIRD_PARTY_NOTICES.md"):
        shutil.copy2(repository_directory / filename, asset_directory / filename)
    collect_rust_licenses(
        repository_directory,
        "hebbian-station",
        asset_directory / "licenses/rust",
        host_target(),
    )
    if include_viewer:
        viewer_directory = asset_directory / "dist/viewer"
        shutil.copytree(repository_directory / "dist/viewer", viewer_directory)
        shutil.rmtree(viewer_directory / "robot", ignore_errors=True)
        shutil.copytree(asset_directory / "robots", viewer_directory / "robot")
        collect_viewer_licenses(
            repository_directory, asset_directory / "licenses/viewer"
        )
        shutil.copy2(
            repository_directory / "licenses/viewer/README.md",
            asset_directory / "licenses/viewer/README.md",
        )


@contextmanager
def fresh_staging_directory(directory: Path) -> Generator[Path]:
    # Never delete staging owned by an earlier or concurrent build.
    directory.mkdir()
    try:
        yield directory
    finally:
        shutil.rmtree(directory)


def build_local(
    repository_directory: Path, *, include_viewer: bool, include_media: bool
) -> None:
    with ExitStack() as staging:
        server_staging = staging.enter_context(
            fresh_staging_directory(repository_directory / "station/wheel-data")
        )
        media_staging = (
            staging.enter_context(
                fresh_staging_directory(
                    repository_directory / "livekit-cameras/wheel-data"
                )
            )
            if include_media
            else None
        )
        if include_viewer:
            subprocess.run(["pnpm", "build"], check=True)
        stage_server_assets(
            repository_directory, server_staging, include_viewer=include_viewer
        )
        if media_staging is not None:
            media_assets = media_staging / "data/share/hebbian-robot-use-media"
            media_assets.mkdir(parents=True)
            shutil.copy2(repository_directory / "LICENSE", media_assets / "LICENSE")
            shutil.copy2(
                repository_directory / "licenses/media-notices.md",
                media_assets / "THIRD_PARTY_NOTICES.md",
            )
            shutil.copytree(
                repository_directory / "licenses/recording-schema",
                media_assets / "licenses/recording-schema",
            )
            collect_rust_licenses(
                repository_directory,
                "livekit-cameras",
                media_assets / "licenses/rust",
                host_target(),
            )
        build_arguments = [
            "uvx",
            "--from",
            "maturin>=1.7,<2",
            "maturin",
            "build",
            "--locked",
            "--release",
            "--compatibility",
            "off",
            "--out",
            str(repository_directory / "target/local-wheels"),
        ]
        subprocess.run(
            [*build_arguments, "--manifest-path", "station/Cargo.toml"], check=True
        )
        if media_staging is not None:
            media_environment = os.environ | {
                "CXX": os.environ.get("CXX", "clang++-21")
            }
            with tempfile.TemporaryDirectory(
                prefix="rua-media-build-"
            ) as temporary_directory:
                build_messages = Path(temporary_directory) / "messages.jsonl"
                with build_messages.open("w") as output:
                    subprocess.run(
                        [
                            "cargo",
                            "build",
                            "--locked",
                            "--release",
                            "-p",
                            "livekit-cameras",
                            "--message-format=json",
                        ],
                        stdout=output,
                        env=media_environment,
                        check=True,
                    )
                collect_webrtc_license(
                    build_messages,
                    media_staging
                    / "data/share/hebbian-robot-use-media/licenses/webrtc",
                )
            subprocess.run(
                [*build_arguments, "--manifest-path", "livekit-cameras/Cargo.toml"],
                env=media_environment,
                check=True,
            )


def build_manylinux(repository_directory: Path, *, include_viewer: bool) -> None:
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("This build requires an x86-64 Linux host with Docker")
    if include_viewer:
        subprocess.run(["pnpm", "build"], check=True)
    with tempfile.TemporaryDirectory(prefix="rua-manylinux-") as temporary_directory:
        source_directory = Path(temporary_directory) / "source"
        source_directory.mkdir()
        for relative_path in tracked_files(
            repository_directory, include_untracked=True
        ):
            source_path = repository_directory / relative_path
            if not source_path.is_file():
                continue
            destination = source_directory / relative_path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source_path, destination, follow_symlinks=False)
        stage_server_assets(
            repository_directory,
            source_directory / "station/wheel-data",
            include_viewer=include_viewer,
        )
        cache_directory = repository_directory / "target/rua-manylinux-cache"
        output_directory = repository_directory / "target/manylinux-wheels"
        for directory in (
            cache_directory / "cargo",
            cache_directory / "target",
            output_directory,
        ):
            directory.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--entrypoint",
                "bash",
                "-v",
                f"{source_directory}:/work/source:ro",
                "-v",
                f"{cache_directory}/cargo:/work/cargo",
                "-v",
                f"{cache_directory}/target:/work/target",
                "-v",
                f"{output_directory}:/work/wheels",
                "-e",
                "CARGO_HOME=/work/cargo",
                "-e",
                "CARGO_TARGET_DIR=/work/target",
                "-e",
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-C debuginfo=0",
                "-e",
                f"RUA_HOST_UID={os.getuid()}",
                "-e",
                f"RUA_HOST_GID={os.getgid()}",
                MANYLINUX_IMAGE,
                "-lc",
                "trap 'chown -R \"$RUA_HOST_UID:$RUA_HOST_GID\" /work/cargo /work/target /work/wheels' EXIT\n"
                "maturin build --manifest-path /work/source/station/Cargo.toml --locked --release "
                "--compatibility manylinux2014 --auditwheel check --out /work/wheels",
            ],
            check=True,
        )


def build_packages(
    repository_directory: Path,
    build_platform: BuildPlatform,
    *,
    include_viewer: bool,
    include_media: bool,
) -> None:
    prepare_robot_assets(repository_directory)
    check_release_versions(repository_directory)
    match build_platform:
        case BuildPlatform.LOCAL:
            build_local(
                repository_directory,
                include_viewer=include_viewer,
                include_media=include_media,
            )
        case BuildPlatform.MANYLINUX:
            build_manylinux(repository_directory, include_viewer=include_viewer)
