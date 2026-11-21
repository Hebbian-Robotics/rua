import os
import platform
import shutil
import subprocess
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path

from rua_dev.licenses.rust import collect_rust_licenses
from rua_dev.licenses.webrtc import collect_webrtc_license
from rua_dev.robot_assets import prepare_robot_assets
from rua_dev.versions import check_release_versions


def tracked_assets(repository: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "--", "profiles", "robots"],
        cwd=repository,
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
    repository_directory: Path, staging_directory: Path, target: str
) -> None:
    robot_assets = prepare_robot_assets(repository_directory)
    asset_directory = staging_directory / "data/share/hebbian-robot-use"
    for directory in ("profiles", "robots"):
        (asset_directory / directory).mkdir(parents=True, exist_ok=True)
    for asset in tracked_assets(repository_directory):
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
        "hebbian-robot-use-server",
        asset_directory / "licenses/rust",
        target,
    )


@contextmanager
def fresh_staging_directory(directory: Path) -> Generator[Path]:
    # INVARIANT: Never delete staging owned by an earlier or concurrent build.
    directory.mkdir()
    try:
        yield directory
    finally:
        shutil.rmtree(directory)


def build_component_wheels(repository: Path, output_directory: Path) -> None:
    check_release_versions(repository)
    target = host_target()
    with (
        fresh_staging_directory(repository / "station/wheel-data") as server_staging,
        fresh_staging_directory(
            repository / "livekit-cameras/wheel-data"
        ) as media_staging,
    ):
        stage_server_assets(repository, server_staging, target)
        media_assets = media_staging / "data/share/hebbian-robot-use-media"
        media_assets.mkdir(parents=True, exist_ok=True)
        shutil.copy2(repository / "LICENSE", media_assets / "LICENSE")
        shutil.copy2(
            repository / "licenses/media-notices.md",
            media_assets / "THIRD_PARTY_NOTICES.md",
        )
        shutil.copytree(
            repository / "licenses/recording-schema",
            media_assets / "licenses/recording-schema",
        )
        collect_rust_licenses(
            repository, "livekit-cameras", media_assets / "licenses/rust", target
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
            str(output_directory),
        ]
        subprocess.run(
            [*build_arguments, "--manifest-path", "station/Cargo.toml"],
            cwd=repository,
            check=True,
        )
        media_environment = os.environ | {
            "CXX": os.environ.get(
                "CXX", "clang++" if platform.system() == "Darwin" else "clang++-21"
            )
        }
        with tempfile.TemporaryDirectory(prefix="rua-media-build-") as temporary:
            build_messages = Path(temporary) / "messages.jsonl"
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
                    cwd=repository,
                    stdout=output,
                    env=media_environment,
                    check=True,
                )
            collect_webrtc_license(build_messages, media_assets / "licenses/webrtc")
        subprocess.run(
            [*build_arguments, "--manifest-path", "livekit-cameras/Cargo.toml"],
            cwd=repository,
            env=media_environment,
            check=True,
        )


def built_wheel(directory: Path, distribution: str, version: str) -> Path:
    candidates = sorted(directory.glob(f"{distribution}-{version}-*.whl"))
    if len(candidates) != 1:
        raise ValueError(
            f"Expected one {distribution} {version} wheel in {directory}; found {len(candidates)}"
        )
    return candidates[0]
