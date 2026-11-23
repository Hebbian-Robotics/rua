import hashlib
import subprocess
from pathlib import Path
from zipfile import ZipFile

import pytest
import tomli_w
from rua_dev.bundle import assemble_bundle
from rua_dev.generated import check_generated_files


@pytest.mark.parametrize(
    "change", ["unstaged", "staged", "staged-with-clean-worktree", "untracked"]
)
def test_generated_file_check_rejects_uncommitted_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    # WHY: Use a real repository: the integration risk is overlooking staged or new outputs.
    subprocess.run(["git", "init", "--quiet", str(tmp_path)], check=True)
    generated_path = tmp_path / "generated.txt"
    generated_path.write_text("committed\n")
    subprocess.run(["git", "add", "generated.txt"], cwd=tmp_path, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "Fixture",
        ],
        cwd=tmp_path,
        check=True,
    )
    monkeypatch.chdir(tmp_path)
    (tmp_path / "unrelated.txt").write_text("unrelated work")
    check_generated_files([Path("generated.txt")])
    match change:
        case "unstaged" | "staged" | "staged-with-clean-worktree":
            generated_path.write_text("changed\n")
            if change in {"staged", "staged-with-clean-worktree"}:
                subprocess.run(["git", "add", "generated.txt"], check=True)
            if change == "staged-with-clean-worktree":
                generated_path.write_text("committed\n")
            with pytest.raises(subprocess.CalledProcessError):
                check_generated_files([Path("generated.txt")])
        case "untracked":
            (tmp_path / "new-generated.txt").write_text("new output\n")
            with pytest.raises(RuntimeError, match="Generated files are not committed"):
                check_generated_files(
                    [Path("generated.txt"), Path("new-generated.txt")]
                )


@pytest.mark.parametrize("matching_wheels", [0, 1, 2])
def test_native_bundle_requires_one_unambiguous_build_artifact(
    tmp_path: Path, matching_wheels: int
) -> None:
    from rua_dev.packaging import built_wheel

    for architecture in ["x86_64", "aarch64"][:matching_wheels]:
        (
            tmp_path
            / f"hebbian_robot_use_server-0.1.0-py3-none-linux_{architecture}.whl"
        ).touch()
    (tmp_path / "hebbian_robot_use_server-0.0.9-py3-none-linux_x86_64.whl").touch()
    (tmp_path / "hebbian_robot_use_media-0.1.0-py3-none-linux_x86_64.whl").touch()
    if matching_wheels == 1:
        assert built_wheel(tmp_path, "hebbian_robot_use_server", "0.1.0").name == (
            "hebbian_robot_use_server-0.1.0-py3-none-linux_x86_64.whl"
        )
    else:
        with pytest.raises(ValueError, match="Expected one hebbian_robot_use_server"):
            built_wheel(tmp_path, "hebbian_robot_use_server", "0.1.0")


def test_failed_bundle_assembly_preserves_previous_release(tmp_path: Path) -> None:
    repository = tmp_path / "source"
    repository.mkdir()
    (repository / "pyproject.toml").write_text(
        '[project]\nname = "hebbian-rua"\nversion = "0.1.0"\n'
        '[build-system]\nrequires = ["uv_build>=0.13.6,<0.14"]\n'
        'build-backend = "uv_build"\n'
    )
    (repository / "hebbian_rua").mkdir()
    (repository / "hebbian_rua/__init__.py").write_text("")
    (repository / "schemas").mkdir()
    (repository / "schemas/sim-plant.schema.json").write_text("{}")
    (repository / "robots").mkdir()
    model_assets = {
        "robot.urdf": b"stock robot geometry",
        "LICENSE.txt": b"model license",
        "README.md": b"model provenance",
        "soft-gripper-camera/finger.stl": b"soft gripper geometry",
    }
    (repository / "robots/so101-assets.toml").write_text(
        tomli_w.dumps(
            {
                "repository": "fixture/so101",
                "revision": "0" * 40,
                "asset_directory": "robot",
                "files": {
                    name: hashlib.sha256(content).hexdigest()
                    for name, content in model_assets.items()
                },
            }
        )
    )
    (repository / "profiles").mkdir()
    for filename in ("README.md", "LICENSE"):
        (repository / filename).write_text("Release fixture\n")
    server_wheel = tmp_path / "hebbian_robot_use_server-0.1.0-py3-none-linux_x86_64.whl"
    media_wheel = tmp_path / "hebbian_robot_use_media-0.1.0-py3-none-linux_x86_64.whl"
    with ZipFile(server_wheel, "w") as archive:
        archive.writestr(
            "hebbian_robot_use_server-0.1.0.data/scripts/hebbian-robot-use-server",
            b"server executable",
        )
        for filename, content in model_assets.items():
            archive.writestr(
                f"hebbian_robot_use_server-0.1.0.data/data/share/hebbian-robot-use/robots/so101/{filename}",
                content,
            )
    with ZipFile(media_wheel, "w") as archive:
        archive.writestr(
            "hebbian_robot_use_media-0.1.0.data/scripts/hebbian-robot-use-media",
            b"media executable",
        )
    output_directory = tmp_path / "dist"
    sdk_wheel = tmp_path / "hebbian_rua-0.1.0-cp312-abi3-linux_x86_64.whl"
    with ZipFile(sdk_wheel, "w") as archive:
        archive.writestr(
            "hebbian_rua-0.1.0.dist-info/WHEEL", "Tag: cp312-abi3-linux_x86_64\n"
        )
    release = assemble_bundle(
        repository, sdk_wheel, server_wheel, media_wheel, output_directory
    )
    previous_release = release.read_bytes()
    with ZipFile(release) as archive:
        assert (
            archive.read("hebbian_rua-0.1.0.data/scripts/hebbian-robot-use-media")
            == b"media executable"
        )
        model_prefix = (
            "hebbian_rua-0.1.0.data/data/share/hebbian-robot-use/robots/so101/"
        )
        assert archive.read(model_prefix + "robot.urdf") == b"stock robot geometry"
        assert (
            archive.read(model_prefix + "soft-gripper-camera/finger.stl")
            == b"soft gripper geometry"
        )
    with ZipFile(media_wheel, "w") as archive:
        archive.writestr(
            "hebbian_robot_use_media-0.1.0.data/scripts/hebbian-robot-use-server",
            b"conflicting executable",
        )
    with pytest.raises(ValueError, match="Conflicting bundled wheel file"):
        assemble_bundle(
            repository, sdk_wheel, server_wheel, media_wheel, output_directory
        )
    assert release.read_bytes() == previous_release
    assert list(output_directory.iterdir()) == [release]
