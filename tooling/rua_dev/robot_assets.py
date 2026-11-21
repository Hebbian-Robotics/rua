from __future__ import annotations

import hashlib
import re
import tempfile
import tomllib
import urllib.request
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from zipfile import BadZipFile, ZipFile

MAXIMUM_ARCHIVE_BYTES = 64 * 1024 * 1024


def parse_digest(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("Asset checksums must be lowercase SHA-256 digests")
    return value


@dataclass(frozen=True)
class RobotAssetSource:
    repository: str
    revision: str
    asset_directory: str
    files: dict[str, str]

    @property
    def archive_url(self) -> str:
        return f"https://codeload.github.com/{self.repository}/zip/{self.revision}"

    @property
    def archive_stem(self) -> str:
        return f"{self.repository.split('/')[1]}-{self.revision}"

    @classmethod
    def load(cls, manifest_path: Path) -> RobotAssetSource:
        manifest = tomllib.loads(manifest_path.read_text())
        if set(manifest) != {
            "repository",
            "revision",
            "asset_directory",
            "files",
        }:
            raise ValueError(f"Unexpected asset manifest fields: {manifest_path}")
        repository = manifest["repository"]
        revision = manifest["revision"]
        asset_directory = manifest["asset_directory"]
        file_digests = manifest["files"]
        if (
            not isinstance(repository, str)
            or re.fullmatch(r"[A-Za-z0-9-]+/[A-Za-z0-9_.-]+", repository) is None
            or repository.split("/")[1] in {".", ".."}
        ):
            raise ValueError(
                "Robot asset repository must be a GitHub owner and repository"
            )
        if (
            not isinstance(revision, str)
            or re.fullmatch(r"[0-9a-f]{40}", revision) is None
        ):
            raise ValueError("Robot asset revision must be a full Git commit")
        if (
            not isinstance(asset_directory, str)
            or "\\" in asset_directory
            or any(part in {"", ".", ".."} for part in asset_directory.split("/"))
        ):
            raise ValueError("Robot asset directory must be a relative path")
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
                not in {".urdf", ".stl", ".txt", ".md", ".toml"}
            ):
                raise ValueError(f"Unsupported robot asset filename: {filename}")
            files[filename] = parse_digest(digest)
        return cls(repository, revision, asset_directory, files)

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


def extract_assets(source: RobotAssetSource, archive: Path, destination: Path) -> None:
    with ZipFile(archive) as source_archive:
        for filename, digest in source.files.items():
            entry = source_archive.getinfo(
                f"{source.archive_stem}/{source.asset_directory}/{filename}"
            )
            if entry.file_size > MAXIMUM_ARCHIVE_BYTES:
                raise ValueError(
                    f"Robot asset exceeds the extraction size limit: {filename}"
                )
            content = source_archive.read(entry)
            if hashlib.sha256(content).hexdigest() != digest:
                raise ValueError(
                    f"Robot asset checksum does not match the pin: {filename}"
                )
            staged_asset = destination / filename
            staged_asset.parent.mkdir(parents=True, exist_ok=True)
            staged_asset.write_bytes(content)


def download_archive(url: str, destination: Path) -> None:
    with (
        urllib.request.urlopen(url, timeout=60) as response,
        destination.open("wb") as output,
    ):
        remaining_bytes = MAXIMUM_ARCHIVE_BYTES
        while chunk := response.read(min(1024 * 1024, remaining_bytes + 1)):
            remaining_bytes -= len(chunk)
            if remaining_bytes < 0:
                raise ValueError("Robot asset archive exceeds the download size limit")
            output.write(chunk)


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
    archive = cache / f"{source.archive_stem}.zip"
    with tempfile.TemporaryDirectory(prefix="prepare-", dir=cache) as temporary:
        staging = Path(temporary)
        extracted = staging / "assets"
        try:
            extract_assets(source, archive, extracted)
        except (FileNotFoundError, BadZipFile, KeyError, ValueError):
            downloaded_archive = staging / "download.zip"
            download_archive(source.archive_url, downloaded_archive)
            try:
                extract_assets(source, downloaded_archive, extracted)
            except (BadZipFile, KeyError) as error:
                raise ValueError(
                    "Invalid robot asset archive for the pinned Git revision"
                ) from error
            downloaded_archive.replace(archive)
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
