import hashlib
import platform
import tarfile
import tempfile
import tomllib
from pathlib import Path

from rua_dev.robot_assets import download_archive


def stage_so101_cli(repository: Path, destination: Path) -> None:
    manifest = tomllib.loads((repository / "robots/so101-cli.toml").read_text())
    platform_names = {
        ("Linux", "x86_64"): "so101-linux-x64.tar.gz",
        ("Darwin", "arm64"): "so101-macos-arm64.tar.gz",
    }
    platform_key = (platform.system(), platform.machine())
    if platform_key not in platform_names:
        raise ValueError(f"No SO101 release for {platform_key}")
    archive_name = platform_names[platform_key]
    expected_digest = manifest["archives"][archive_name]
    url = f"https://github.com/Hebbian-Robotics/so101/releases/download/v{manifest['version']}/{archive_name}"
    with tempfile.TemporaryDirectory(prefix="rua-so101-") as temporary:
        archive_path = Path(temporary) / archive_name
        download_archive(url, archive_path)
        if hashlib.sha256(archive_path.read_bytes()).hexdigest() != expected_digest:
            raise ValueError(f"SO101 archive checksum mismatch: {archive_name}")
        with tarfile.open(archive_path, "r:gz") as archive:
            for filename in ("so101", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt"):
                member = archive.getmember(f"./{filename}")
                if not member.isfile() or member.size > 64 * 1024 * 1024:
                    raise ValueError(f"Invalid SO101 archive member: {filename}")
                source = archive.extractfile(member)
                if source is None:
                    raise ValueError(f"Missing SO101 archive member: {filename}")
                if filename == "so101":
                    output = destination / "scripts/so101"
                else:
                    output = destination / "data/share/hebbian-rua-so101" / filename
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(source.read())
                if filename == "so101":
                    output.chmod(0o755)
