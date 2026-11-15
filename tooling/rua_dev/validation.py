"""Validation lanes shared by local development and CI."""

import os
import subprocess
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from rua_dev.models import ValidationLane
from rua_dev.native_build import build_executor
from rua_dev.robot_assets import prepare_robot_assets


@dataclass(frozen=True)
class ViewerChecks:
    check: Callable[[], None]
    check_contracts: Callable[[], None]


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


def prepare_executor() -> None:
    os.environ["HEBBIAN_ROBOT_USE_SERVER"] = str(build_executor())


def check_rust() -> None:
    for arguments in (
        ["cargo", "fmt", "--check"],
        ["cargo", "clippy", "--all-targets", "--all-features"],
        ["cargo", "test", "--locked"],
    ):
        subprocess.run(arguments, check=True)


def check_python_tests(lane: ValidationLane, *, executor: bool) -> None:
    project = tomllib.loads(Path("pyproject.toml").read_text())
    configuration = project.get("tool", {}).get("pytest", {}).get("ini_options", {})
    markers = {
        marker.split(":", 1)[0].strip() for marker in configuration.get("markers", [])
    }
    if executor and "executor" not in markers:
        return
    selection = []
    if "executor" in markers:
        selection.append("executor" if executor else "not executor")
    if lane != ValidationLane.FULL and "typescript" in markers:
        selection.append("not typescript")
    if "contracts" in markers:
        selection.append("not contracts")
    report_directory = Path(".cache/validation")
    report_directory.mkdir(parents=True, exist_ok=True)
    test_lane = "executor" if executor else "fast"
    # Public checkouts do not require xdist; only override workers when the
    # workspace's pytest configuration actually enables them.
    parallel_tests = "-n" in configuration.get("addopts", "")
    subprocess.run(
        [
            "pytest",
            *(["-n", "0"] if executor and parallel_tests else []),
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
    viewer: ViewerChecks | None = None,
) -> None:
    if lane == ValidationLane.VIEWER and viewer is None:
        raise ValueError("This checkout does not include viewer validation")
    prepare_robot_assets(Path.cwd())
    if lane in {ValidationLane.PYTHON, ValidationLane.FAST, ValidationLane.FULL}:
        check_python_static()
    if viewer is not None and lane in {
        ValidationLane.VIEWER,
        ValidationLane.FAST,
        ValidationLane.FULL,
    }:
        viewer.check()
    if lane in {ValidationLane.EXECUTOR, ValidationLane.CONTRACTS, ValidationLane.FULL}:
        prepare_executor()
    if lane in {ValidationLane.EXECUTOR, ValidationLane.FULL}:
        check_rust()
    if lane in {ValidationLane.CONTRACTS, ValidationLane.FULL}:
        from rua_dev.contracts import available_sdk_targets, generate_contracts

        generate_contracts(available_sdk_targets(), check=True)
        if viewer is not None:
            viewer.check_contracts()
    if lane in {ValidationLane.PYTHON, ValidationLane.FAST, ValidationLane.FULL}:
        check_python_tests(lane, executor=False)
    if lane in {ValidationLane.EXECUTOR, ValidationLane.FULL}:
        check_python_tests(lane, executor=True)
