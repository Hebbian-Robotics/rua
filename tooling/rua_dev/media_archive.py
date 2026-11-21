import gzip
import io
import json
import platform
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path
from zipfile import ZipFile


def add_file(archive: tarfile.TarFile, name: str, contents: bytes, mode: int) -> None:
    entry = tarfile.TarInfo(name)
    entry.size = len(contents)
    entry.mode = mode
    entry.uid = entry.gid = 0
    entry.uname = entry.gname = ""
    entry.mtime = 0
    archive.addfile(entry, io.BytesIO(contents))


def build_ubuntu_media_archive(repository: Path) -> Path:
    operating_system = platform.freedesktop_os_release()
    if (
        operating_system.get("ID") != "ubuntu"
        or operating_system.get("VERSION_ID") != "24.04"
        or platform.machine() != "x86_64"
    ):
        raise ValueError("Build this archive on Ubuntu 24.04 x86-64")

    with (repository / "livekit-cameras/Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["package"]["version"]
    wheel = (
        repository
        / "target/local-wheels"
        / f"hebbian_robot_use_media-{version}-py3-none-linux_x86_64.whl"
    )
    if not wheel.is_file():
        raise FileNotFoundError("Build the local server and media wheels first")
    return package_media_wheel(wheel, repository / "target/release-artifacts", version)


def package_media_wheel(wheel: Path, output_directory: Path, version: str) -> Path:
    output_directory.mkdir(parents=True, exist_ok=True)
    bundle_name = f"hebbian-robot-use-media-{version}-ubuntu24.04-x86_64"
    output_path = output_directory / f"{bundle_name}.tar.gz"
    scripts_prefix = f"hebbian_robot_use_media-{version}.data/scripts/"
    data_prefix = f"hebbian_robot_use_media-{version}.data/data/"
    readme = f"""Hebbian Robot Use media {version}

This binary targets Ubuntu 24.04 x86-64. Install the Ubuntu ffmpeg package,
which supplies the ffmpeg command that recording runs:

    sudo apt install ffmpeg

Add this archive's bin directory to PATH beside hebbian-robot-use-server.
The server starts the media service for cameras and recording. Match the media
and server versions. License information is under share/. This is a release
candidate; review the dependency inventory and native-library obligations
before distributing it.
"""
    with tempfile.NamedTemporaryFile(dir=output_directory, delete=False) as output:
        temporary_path = Path(output.name)
        try:
            with (
                ZipFile(wheel) as source,
                gzip.GzipFile(
                    fileobj=output, mode="wb", filename="", mtime=0
                ) as compressed,
                tarfile.open(fileobj=compressed, mode="w") as archive,
            ):
                add_file(archive, f"{bundle_name}/README.txt", readme.encode(), 0o644)
                included: set[str] = set()
                for entry in sorted(source.infolist(), key=lambda item: item.filename):
                    if entry.is_dir():
                        continue
                    if entry.filename == scripts_prefix + "hebbian-robot-use-media":
                        name = "bin/" + entry.filename.removeprefix(scripts_prefix)
                        mode = 0o755
                    elif entry.filename.startswith(
                        data_prefix + "share/hebbian-robot-use-media/"
                    ):
                        name = entry.filename.removeprefix(data_prefix)
                        mode = 0o644
                    else:
                        continue
                    if ".." in Path(name).parts:
                        raise ValueError(f"Unsafe wheel entry: {entry.filename}")
                    add_file(archive, f"{bundle_name}/{name}", source.read(entry), mode)
                    included.add(name)
                required = {
                    "bin/hebbian-robot-use-media",
                    "share/hebbian-robot-use-media/LICENSE",
                    "share/hebbian-robot-use-media/THIRD_PARTY_NOTICES.md",
                    "share/hebbian-robot-use-media/licenses/rust/manifest.json",
                    "share/hebbian-robot-use-media/licenses/webrtc/LICENSE.md",
                }
                if not required.issubset(included):
                    raise ValueError(
                        f"Media wheel is missing: {sorted(required - included)}"
                    )
                rust_licenses = "share/hebbian-robot-use-media/licenses/rust"
                manifest = json.loads(
                    source.read(f"{data_prefix}{rust_licenses}/manifest.json")
                )
                if not manifest:
                    raise ValueError("Media dependency license manifest is empty")
                missing_files = [
                    f"{rust_licenses}/{package['directory']}/{filename}"
                    for package in manifest
                    for filename in package["files"]
                    if f"{rust_licenses}/{package['directory']}/{filename}"
                    not in included
                ]
                if missing_files:
                    raise ValueError(
                        f"Media wheel is missing listed license files: {missing_files}"
                    )
                unreviewed = [
                    f"{package['name']} {package['version']}"
                    for package in manifest
                    if not package["files"]
                ]
                if unreviewed:
                    print(
                        "License files still need review: " + ", ".join(unreviewed),
                        file=sys.stderr,
                    )
            temporary_path.replace(output_path)
        finally:
            temporary_path.unlink(missing_ok=True)
    return output_path
