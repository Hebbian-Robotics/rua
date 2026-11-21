import json
import subprocess
from pathlib import Path

import pytest
from rua_dev.licenses.rust import collect_rust_licenses


@pytest.mark.parametrize("include_dependency_license", [False, True])
def test_dependency_license_files_are_required_and_preserved(
    tmp_path: Path, include_dependency_license: bool
) -> None:
    repository = tmp_path / "repository"
    dependency = tmp_path / "dependency"
    for package_directory in (repository, dependency):
        (package_directory / "src").mkdir(parents=True)
        (package_directory / "src/lib.rs").write_text("")
    (repository / "Cargo.toml").write_text(
        '[package]\nname = "application"\nversion = "0.1.0"\nedition = "2024"\n'
        '[dependencies]\ndependency = { path = "../dependency" }\n'
        "[workspace]\n"
    )
    (dependency / "Cargo.toml").write_text(
        '[package]\nname = "dependency"\nversion = "1.0.0"\nlicense = "MIT"\n'
    )
    (repository / "LICENSE").write_text("Application license\n")
    license_root = repository / "licenses/rust"
    license_root.mkdir(parents=True)
    (license_root / "README.md").write_text("Dependency license provenance\n")
    (license_root / "manifest.json").write_text("[]\n")
    subprocess.run(
        ["cargo", "generate-lockfile", "--offline"], cwd=repository, check=True
    )
    destination = tmp_path / "wheel-licenses"
    if not include_dependency_license:
        with pytest.raises(ValueError, match=r"dependency 1\.0\.0"):
            collect_rust_licenses(
                repository, "application", destination, "x86_64-unknown-linux-gnu"
            )
        return
    (dependency / "LICENSE").write_text("Dependency license\n")
    collect_rust_licenses(
        repository, "application", destination, "x86_64-unknown-linux-gnu"
    )
    assert (
        destination / "dependency-1.0.0/LICENSE"
    ).read_text() == "Dependency license\n"
    inventory = json.loads((destination / "manifest.json").read_text())
    assert all(entry["files"] for entry in inventory)
