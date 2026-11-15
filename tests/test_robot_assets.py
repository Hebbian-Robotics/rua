"""Robot preparation preserves pinned geometry, attribution and offline operation."""

import hashlib
import io
import urllib.request
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

import pytest
from rua_dev.robot_assets import prepare_robot_assets

ASSET_CONTENTS = {
    "robot.urdf": b'<robot name="fixture"><link name="base"/></robot>\n',
    "base.stl": b"fixture mesh geometry\n",
    "LICENSE.txt": b"fixture model license\n",
    "README.md": b"fixture model attribution\n",
}


def create_asset_source(repository: Path) -> tuple[Path, bytes]:
    (repository / "robots").mkdir()
    archive_buffer = io.BytesIO()
    with ZipFile(archive_buffer, "w", compression=ZIP_DEFLATED) as archive:
        for name, content in ASSET_CONTENTS.items():
            archive.writestr(f"model/{name}", content)
        archive.writestr("unrelated/private.txt", b"not a robot asset")
    archive_content = archive_buffer.getvalue()
    archive_digest = hashlib.sha256(archive_content).hexdigest()
    manifest = repository / "robots/so101-assets.toml"
    manifest.write_text(
        'archive_url = "https://offline.invalid/so101.whl"\n'
        f'archive_sha256 = "{archive_digest}"\n'
        'archive_prefix = "model/"\n\n[files]\n'
        + "".join(
            f'"{name}" = "{hashlib.sha256(content).hexdigest()}"\n'
            for name, content in ASSET_CONTENTS.items()
        )
    )
    cache = repository / ".cache/so101"
    cache.mkdir(parents=True)
    (cache / f"{archive_digest}.whl").write_bytes(archive_content)
    return manifest, archive_content


def test_cached_release_prepares_exact_assets_and_repairs_tampering_offline(
    tmp_path: Path,
) -> None:
    create_asset_source(tmp_path)
    assets = prepare_robot_assets(tmp_path)
    assert {asset.name: asset.read_bytes() for asset in assets} == ASSET_CONTENTS
    assert not (tmp_path / "robots/so101/private.txt").exists()
    (tmp_path / "robots/so101/base.stl").write_bytes(b"unreviewed local edit")
    prepare_robot_assets(tmp_path)
    assert (
        tmp_path / "robots/so101/base.stl"
    ).read_bytes() == b"fixture mesh geometry\n"
    # Once prepared, the runtime files alone are sufficient, even without a cache.
    for cached_archive in (tmp_path / ".cache/so101").iterdir():
        cached_archive.unlink()
    assert {
        asset.name: asset.read_bytes() for asset in prepare_robot_assets(tmp_path)
    } == ASSET_CONTENTS


def test_untrusted_download_does_not_replace_existing_geometry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    create_asset_source(tmp_path)
    destination = tmp_path / "robots/so101"
    destination.mkdir()
    (destination / "robot.urdf").write_bytes(b"previous geometry")
    for cached_archive in (tmp_path / ".cache/so101").iterdir():
        cached_archive.unlink()

    def download_archive(url: str, timeout: int) -> io.BytesIO:
        return io.BytesIO(b"corrupt download")

    monkeypatch.setattr(urllib.request, "urlopen", download_archive)
    with pytest.raises(ValueError, match="archive checksum"):
        prepare_robot_assets(tmp_path)
    assert (destination / "robot.urdf").read_bytes() == b"previous geometry"
    assert sorted(path.name for path in destination.iterdir()) == ["robot.urdf"]


def test_mismatched_asset_pin_rejects_the_entire_update(tmp_path: Path) -> None:
    manifest, _ = create_asset_source(tmp_path)
    destination = tmp_path / "robots/so101"
    destination.mkdir()
    (destination / "robot.urdf").write_bytes(b"previous geometry")
    manifest.write_text(
        manifest.read_text().replace(
            hashlib.sha256(ASSET_CONTENTS["LICENSE.txt"]).hexdigest(), "0" * 64
        )
    )
    with pytest.raises(ValueError, match="asset checksum"):
        prepare_robot_assets(tmp_path)
    assert (destination / "robot.urdf").read_bytes() == b"previous geometry"
    assert sorted(path.name for path in destination.iterdir()) == ["robot.urdf"]


@pytest.mark.parametrize(
    "filename", ["../outside.stl", "/outside.stl", "..\\outside.stl"]
)
def test_asset_pin_cannot_write_outside_its_generated_directory(
    tmp_path: Path, filename: str
) -> None:
    manifest, _ = create_asset_source(tmp_path)
    # Literal TOML keys preserve backslashes for the traversal case.
    manifest.write_text(manifest.read_text() + f"'{filename}' = '{'0' * 64}'\n")
    with pytest.raises(ValueError, match="filename"):
        prepare_robot_assets(tmp_path)
    assert not (tmp_path / "robots/so101").exists()
    assert not (tmp_path / "robots/outside.stl").exists()
