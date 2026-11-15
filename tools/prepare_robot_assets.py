"""Prepare checksum-pinned SO-101 assets before building a source checkout.

This standard-library-only module is also exported as tools/prepare_robot_assets.py.
Installed RUA distributions contain the assets and never run this preparation.
"""

from __future__ import annotations

import argparse
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
                or "/" in filename
                or "\\" in filename
                or filename in {".", ".."}
                or not (
                    filename in {"robot.urdf", "LICENSE.txt", "README.md"}
                    or filename.endswith(".stl")
                )
            ):
                raise ValueError(f"Unsupported robot asset filename: {filename}")
            files[filename] = parse_digest(digest)
        return cls(
            archive_url, parse_digest(manifest["archive_sha256"]), archive_prefix, files
        )

    def matches(self, directory: Path) -> bool:
        return all(
            (directory / filename).is_file()
            and not (directory / filename).is_symlink()
            and hashlib.sha256((directory / filename).read_bytes()).hexdigest()
            == digest
            for filename, digest in self.files.items()
        )


def prepare_robot_assets(repository: Path) -> tuple[Path, ...]:
    """Materialize only pinned assets; a complete checkout needs no network access."""
    repository = repository.resolve()
    source = RobotAssetSource.load(repository / "robots/so101-assets.toml")
    destination = repository / "robots/so101"
    if destination.is_symlink():
        raise ValueError(
            f"Generated asset directory must not be a symlink: {destination}"
        )
    if source.matches(destination):
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
                (extracted / filename).write_bytes(content)
        # Verify everything before touching the runtime directory. Each replacement is
        # atomic; a failed preparation is detected and repaired on the next run.
        destination.mkdir(parents=True, exist_ok=True)
        for filename in source.files:
            if (destination / filename).is_symlink():
                raise ValueError(f"Generated asset must not be a symlink: {filename}")
        for filename in source.files:
            (extracted / filename).replace(destination / filename)
    return tuple(destination / filename for filename in sorted(source.files))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    arguments = parser.parse_args()
    try:
        assets = prepare_robot_assets(arguments.repository)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Could not prepare robot assets: {error}\n")
    print(f"Prepared {len(assets)} pinned SO-101 assets in {assets[0].parent}")


if __name__ == "__main__":
    main()
