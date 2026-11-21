import os
import subprocess
import tomllib
from collections.abc import Callable
from pathlib import Path

from rua_dev.models import ValidationLane
from rua_dev.native_build import build_server
from rua_dev.robot_assets import prepare_robot_assets


def check_python_static() -> None:
    for arguments in (
        ["ruff", "check"],
        ["ruff", "format", "--check"],
        ["ty", "check"],
    ):
        subprocess.run(arguments, check=True)
    project = tomllib.loads(Path("pyproject.toml").read_text())
    if "importlinter" in project.get("tool", {}):
        subprocess.run(["lint-imports"], check=True)


def prepare_server() -> None:
    os.environ["HEBBIAN_ROBOT_USE_SERVER"] = str(build_server())


def check_rust_architecture() -> None:
    manifests = [
        ("server-core/Cargo.toml", "server"),
        ("station/Cargo.toml", "server"),
        *(
            (manifest, "client")
            for manifest in ("sdk/Cargo.toml", "cli/Cargo.toml")
            if Path(manifest).is_file()
        ),
    ]
    for manifest, policy in manifests:
        subprocess.run(
            [
                "cargo",
                "deny",
                "--manifest-path",
                manifest,
                "--config",
                f"tooling/architecture/{policy}.toml",
                "--all-features",
                "--exclude-dev",
                "--locked",
                "check",
                "bans",
            ],
            check=True,
        )


def check_rust() -> None:
    check_rust_architecture()
    for arguments in (
        ["cargo", "fmt", "--check"],
        ["cargo", "machete", "--with-metadata", "--skip-target-dir"],
        [
            "cargo",
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        ["cargo", "test", "--locked", "--workspace", "--all-features"],
    ):
        subprocess.run(arguments, check=True)


def check_python_tests(lane: ValidationLane, *, server: bool) -> None:
    project = tomllib.loads(Path("pyproject.toml").read_text())
    configuration = project.get("tool", {}).get("pytest", {}).get("ini_options", {})
    markers = {
        marker.split(":", 1)[0].strip() for marker in configuration.get("markers", [])
    }
    if server and "server" not in markers:
        return
    selection = []
    if "server" in markers:
        selection.append("server" if server else "not server")
    if lane != ValidationLane.FULL and "typescript" in markers:
        selection.append("not typescript")
    report_directory = Path(".cache/validation")
    report_directory.mkdir(parents=True, exist_ok=True)
    test_lane = "server" if server else "fast"
    subprocess.run(
        [
            "pytest",
            *(["-m", " and ".join(selection)] if selection else []),
            "--durations=25",
            "--durations-min=0.1",
            f"--junitxml={report_directory}/python-{lane}-{test_lane}.xml",
        ],
        check=True,
    )


def validate(
    lane: ValidationLane,
    *,
    check_viewer: Callable[[], None] | None = None,
) -> None:
    if lane == ValidationLane.VIEWER and check_viewer is None:
        raise ValueError("This checkout does not include viewer validation")
    subprocess.run(
        ["source-doc-lint", "--config", "source-doc-lint.toml"],
        check=True,
    )
    prepare_robot_assets(Path.cwd())
    if lane in {ValidationLane.PYTHON, ValidationLane.FAST, ValidationLane.FULL}:
        check_python_static()
    if check_viewer is not None and lane in {
        ValidationLane.VIEWER,
        ValidationLane.FAST,
        ValidationLane.FULL,
    }:
        check_viewer()
    if lane in {ValidationLane.SERVER, ValidationLane.CONTRACTS, ValidationLane.FULL}:
        prepare_server()
    if lane in {ValidationLane.SERVER, ValidationLane.FULL}:
        check_rust()
    if lane in {ValidationLane.CONTRACTS, ValidationLane.FULL}:
        from rua_dev.contracts import available_sdk_targets, generate_contracts

        generate_contracts(available_sdk_targets(), check=True)
    if lane in {ValidationLane.PYTHON, ValidationLane.FAST, ValidationLane.FULL}:
        check_python_tests(lane, server=False)
    if lane in {ValidationLane.SERVER, ValidationLane.FULL}:
        check_python_tests(lane, server=True)
