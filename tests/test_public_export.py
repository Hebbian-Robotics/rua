import json
import subprocess
from pathlib import Path

import pytest
from rua_dev.public_export import export_public


def create_source_checkout(source_directory: Path) -> None:
    source_directory.mkdir()
    template_directory = source_directory / "tooling/public_export"
    (template_directory / "files").mkdir(parents=True)
    (template_directory / "manifest.toml").write_text(
        'paths = ["Cargo.toml", "public", "LICENSE"]\n'
        'exclude = []\nworkspace_members = ["public"]\n'
    )
    (source_directory / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["public"]\nresolver = "3"\n'
    )
    (source_directory / "LICENSE").write_text("Public license\n")
    (source_directory / "public").mkdir()
    (source_directory / "public/src").mkdir()
    (source_directory / "public/src/lib.rs").write_text("")
    (source_directory / "public/Cargo.toml").write_text(
        '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2024"\n'
    )
    (source_directory / "public/source.txt").write_text("Public source\n")
    (source_directory / "public/LICENSE").symlink_to("../LICENSE")
    (source_directory / "private.txt").write_text("Private source\n")
    (template_directory / "files/README.md").write_text("Public instructions\n")
    subprocess.run(["git", "init", "-q", str(source_directory)], check=True)
    subprocess.run(["git", "add", "."], cwd=source_directory, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "Fixture",
        ],
        cwd=source_directory,
        check=True,
    )
    # WHY: These exist under an allowed directory but must never enter a release.
    (source_directory / "public/local-secret.txt").write_text("Untracked secret\n")
    (source_directory / ".env").write_text("SECRET=private\n")
    (template_directory / "files/private.txt").write_text("Untracked overlay secret\n")


def test_export_excludes_private_and_untracked_files_and_preserves_destination(
    tmp_path: Path,
) -> None:
    source_directory = tmp_path / "source"
    create_source_checkout(source_directory)
    with pytest.raises(ValueError, match="outside"):
        export_public(source_directory, source_directory / "nested-export")
    destination = tmp_path / "export"
    export_public(source_directory, destination)
    assert (destination / "public/source.txt").read_text() == "Public source\n"
    assert (destination / "public/LICENSE").read_text() == "Public license\n"
    assert not (destination / "public/LICENSE").is_symlink()
    assert not (destination / "public/local-secret.txt").exists()
    assert not (destination / "private.txt").exists()
    assert not (destination / ".env").exists()
    assert not (destination / ".git").exists()
    assert (destination / "README.md").read_text() == "Public instructions\n"
    provenance = json.loads((destination / "EXPORT.json").read_text())
    assert provenance["source_dirty"] is True
    with pytest.raises(ValueError, match="already exists"):
        export_public(source_directory, destination)
    assert (destination / "public/source.txt").read_text() == "Public source\n"


def test_export_refuses_symlink_to_private_content(tmp_path: Path) -> None:
    source_directory = tmp_path / "source"
    create_source_checkout(source_directory)
    (source_directory / "public/source.txt").unlink()
    (source_directory / "public/source.txt").symlink_to("../private.txt")
    destination = tmp_path / "export"
    with pytest.raises(ValueError, match="regular file"):
        export_public(source_directory, destination)
    assert not destination.exists()


def test_export_flattens_explicit_shared_assets_without_exporting_their_package(
    tmp_path: Path,
) -> None:
    source_directory = tmp_path / "source"
    create_source_checkout(source_directory)
    asset_directory = source_directory / "shared-assets"
    asset_directory.mkdir()
    (asset_directory / "model.stl").write_bytes(b"robot geometry")
    (source_directory / "public/model.stl").symlink_to("../shared-assets/model.stl")
    manifest_path = source_directory / "tooling/public_export/manifest.toml"
    manifest_path.write_text(
        manifest_path.read_text() + '\nsymlink_targets = ["shared-assets"]\n'
    )
    subprocess.run(
        ["git", "add", "shared-assets/model.stl", "public/model.stl"],
        cwd=source_directory,
        check=True,
    )
    destination = tmp_path / "export"
    export_public(source_directory, destination)
    assert (destination / "public/model.stl").read_bytes() == b"robot geometry"
    assert not (destination / "public/model.stl").is_symlink()
    assert not (destination / "shared-assets").exists()


def test_export_keeps_package_identity_and_shared_checks_while_pruning_dev_components(
    tmp_path: Path,
) -> None:
    import tomllib

    import tomli_w

    source = tmp_path / "source"
    create_source_checkout(source)
    configuration = {
        "project": {
            "name": "hebbian-rua",
            "version": "0.1.0",
            "requires-python": ">=3.12",
            "dependencies": [],
        },
        "dependency-groups": {
            "dev": ["rua-shared", "rua-internal", "import-linter"],
            "policy": ["rua-internal"],
        },
        "tool": {
            "uv": {
                "package": False,
                "workspace": {"members": ["shared", "internal"]},
                "sources": {
                    "rua-shared": {"workspace": True},
                    "rua-internal": {"workspace": True},
                },
            },
            "ruff": {"target-version": "py312", "lint": {"select": ["E", "F", "ANN"]}},
            "ty": {
                "rules": {"division-by-zero": "error"},
                "environment": {"extra-paths": ["shared", "internal"]},
            },
            "pytest": {
                "ini_options": {
                    "testpaths": ["tests"],
                    "addopts": "-q",
                    "markers": ["server: native fixture"],
                }
            },
            "importlinter": {"root_packages": ["internal"]},
        },
    }
    (source / "pyproject.toml").write_text(tomli_w.dumps(configuration))
    for directory, name in (("shared", "rua-shared"), ("internal", "rua-internal")):
        (source / directory).mkdir()
        (source / directory / "pyproject.toml").write_text(
            f'[project]\nname = "{name}"\nversion = "0.1.0"\n[tool.uv]\npackage = false\n'
        )
    manifest = source / "tooling/public_export/manifest.toml"
    manifest.write_text(
        manifest.read_text().replace(
            '"LICENSE"]', '"LICENSE", "pyproject.toml", "shared"]'
        )
        + '\npython_excluded_dependencies = ["rua-internal", "import-linter"]\n'
        'python_excluded_groups = ["policy"]\n'
    )
    subprocess.run(
        ["git", "add", "pyproject.toml", "shared", "internal"], cwd=source, check=True
    )
    destination = tmp_path / "export"
    export_public(source, destination)
    exported = tomllib.loads((destination / "pyproject.toml").read_text())
    assert exported["project"] == configuration["project"]
    assert exported["tool"]["ruff"] == configuration["tool"]["ruff"]
    assert exported["tool"]["ty"]["rules"] == configuration["tool"]["ty"]["rules"]
    assert exported["tool"]["ty"]["environment"]["extra-paths"] == ["shared"]
    assert exported["dependency-groups"] == {"dev": ["rua-shared"]}
    assert exported["tool"]["uv"]["workspace"]["members"] == ["shared"]
    assert exported["tool"]["uv"]["sources"] == {"rua-shared": {"workspace": True}}
    assert exported["tool"]["pytest"]["ini_options"] == {
        "testpaths": ["tests"],
        "addopts": "-q",
    }
    assert "importlinter" not in exported["tool"]
    assert not (destination / "internal").exists()
