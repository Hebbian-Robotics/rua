from __future__ import annotations

import hashlib
import re
import tempfile
import tomllib
import urllib.request
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from zipfile import ZipFile

MAXIMUM_ARCHIVE_BYTES = 64 * 1024 * 1024


def parse_digest(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("Asset checksums must be lowercase SHA-256 digests")
    return value


@dataclass(frozen=True)
class RobotAssetSource:
    archive_url: str
    archive_sha256: str
    archive_prefix: str
    files: dict[str, str]

    @classmethod
    def load(cls, manifest_path: Path) -> RobotAssetSource:
        manifest = tomllib.loads(manifest_path.read_text())
        if set(manifest) != {
            "archive_url",
            "archive_sha256",
            "archive_prefix",
            "files",
        }:
            raise ValueError(f"Unexpected asset manifest fields: {manifest_path}")
        archive_url = manifest["archive_url"]
        archive_prefix = manifest["archive_prefix"]
        file_digests = manifest["files"]
        if not isinstance(archive_url, str) or not archive_url.startswith("https://"):
            raise ValueError("Robot asset archives require an HTTPS URL")
        if (
            not isinstance(archive_prefix, str)
            or not archive_prefix.endswith("/")
            or PurePosixPath(archive_prefix).is_absolute()
            or ".." in PurePosixPath(archive_prefix).parts
        ):
            raise ValueError("Robot asset archive prefix must be a relative directory")
        if not isinstance(file_digests, dict) or not {
            "robot.urdf",
            "LICENSE.txt",
            "README.md",
        }.issubset(file_digests):
            raise ValueError("Robot assets require a URDF, license and attribution")
        files: dict[str, str] = {}
        for filename, digest in file_digests.items():
            if (
                not isinstance(filename, str)
                or "\\" in filename
                or PurePosixPath(filename).is_absolute()
                or any(part in {"", ".", ".."} for part in filename.split("/"))
                or PurePosixPath(filename).suffix
                not in {".urdf", ".stl", ".txt", ".md"}
            ):
                raise ValueError(f"Unsupported robot asset filename: {filename}")
            files[filename] = parse_digest(digest)
        return cls(
            archive_url, parse_digest(manifest["archive_sha256"]), archive_prefix, files
        )

    def matches(self, directory: Path) -> bool:
        return all(
            asset_path(directory, filename).is_file()
            and hashlib.sha256(asset_path(directory, filename).read_bytes()).hexdigest()
            == digest
            for filename, digest in self.files.items()
        )


def asset_path(directory: Path, filename: str) -> Path:
    path = directory
    for part in PurePosixPath(filename).parts:
        path = path / part
        # CONSTRAINT: Nested symlinks must not redirect generated assets outside
        # their intended directory.
        if path.is_symlink():
            raise ValueError(f"Generated asset path must not contain a symlink: {path}")
    return path


def prepare_robot_assets(repository: Path) -> tuple[Path, ...]:
    repository = repository.resolve()
    source = RobotAssetSource.load(repository / "robots/so101-assets.toml")
    destination = repository / "robots/so101"
    if destination.is_symlink():
        raise ValueError(
            f"Generated asset directory must not be a symlink: {destination}"
        )
    if source.matches(destination):
        # WHY: A complete pinned checkout does not need network access.
        return tuple(destination / filename for filename in sorted(source.files))
    cache = repository / ".cache/so101"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"{source.archive_sha256}.whl"
    with tempfile.TemporaryDirectory(prefix="prepare-", dir=cache) as temporary:
        staging = Path(temporary)
        if (
            not archive.is_file()
            or hashlib.sha256(archive.read_bytes()).hexdigest() != source.archive_sha256
        ):
            downloaded_archive = staging / "download.whl"
            with (
                urllib.request.urlopen(source.archive_url, timeout=60) as response,
                downloaded_archive.open("wb") as output,
            ):
                remaining_bytes = MAXIMUM_ARCHIVE_BYTES
                while chunk := response.read(min(1024 * 1024, remaining_bytes + 1)):
                    remaining_bytes -= len(chunk)
                    if remaining_bytes < 0:
                        raise ValueError(
                            "Robot asset archive exceeds the download size limit"
                        )
                    output.write(chunk)
            if (
                hashlib.sha256(downloaded_archive.read_bytes()).hexdigest()
                != source.archive_sha256
            ):
                raise ValueError(
                    "Robot asset archive checksum does not match the pinned release"
                )
            downloaded_archive.replace(archive)
        extracted = staging / "assets"
        extracted.mkdir()
        with ZipFile(archive) as wheel:
            for filename, digest in source.files.items():
                entry = wheel.getinfo(source.archive_prefix + filename)
                if entry.file_size > MAXIMUM_ARCHIVE_BYTES:
                    raise ValueError(
                        f"Robot asset exceeds the extraction size limit: {filename}"
                    )
                content = wheel.read(entry)
                if hashlib.sha256(content).hexdigest() != digest:
                    raise ValueError(
                        f"Robot asset checksum does not match the pin: {filename}"
                    )
                staged_asset = extracted / filename
                staged_asset.parent.mkdir(parents=True, exist_ok=True)
                staged_asset.write_bytes(content)
        # INVARIANT: Verify everything before touching the runtime directory. Each replacement is
        # atomic; a failed preparation is detected and repaired on the next run.
        destination.mkdir(parents=True, exist_ok=True)
        for filename in source.files:
            asset_path(destination, filename)
        for filename in source.files:
            destination_asset = asset_path(destination, filename)
            destination_asset.parent.mkdir(parents=True, exist_ok=True)
            (extracted / filename).replace(destination_asset)
    return tuple(destination / filename for filename in sorted(source.files))
