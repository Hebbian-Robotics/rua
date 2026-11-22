import importlib
import importlib.util
import os
import subprocess
import sys
from pathlib import Path
from typing import Annotated

from cyclopts import App, Parameter

from rua_dev.models import SdkTarget, ValidationLane


def setup() -> None:
    from rua_dev.robot_assets import prepare_robot_assets

    assets = prepare_robot_assets(Path.cwd())
    print(f"Prepared {len(assets)} SO-101 assets in {assets[0].parent}")


def contracts(
    *,
    targets: Annotated[
        tuple[SdkTarget, ...], Parameter(consume_multiple=True, negative=False)
    ] = (),
    check: bool = False,
) -> None:
    from rua_dev.contracts import available_sdk_targets, generate_contracts

    generate_contracts(frozenset(targets) or available_sdk_targets(), check=check)


def check_architecture() -> None:
    from rua_dev.validation import check_rust_architecture

    check_rust_architecture()


def validate(
    *,
    lane: ValidationLane = ValidationLane.FULL,
) -> None:
    from rua_dev.validation import validate as run_validation

    run_validation(lane)


def package_check(wheel: Path) -> None:
    from rua_dev.wheel_check import check_wheel

    check_wheel(wheel.resolve())


def check_release_versions(*, tag: str | None = None) -> None:
    from rua_dev.versions import check_release_versions as verify_versions

    verify_versions(Path.cwd(), tag=tag)


def package_rua() -> None:
    from rua_dev.bundle import build_rua_bundle

    print(build_rua_bundle(Path.cwd()))


def create_app() -> App:
    app = App(help="Development commands for a RUA source checkout.")
    package_app = App(name="package", help="Wheel distribution commands.")
    package_app.command(
        package_check,
        name="check",
        help="Install a release wheel in an empty environment and check real commands.",
    )
    app.command(package_app)
    app.command(setup, help="Prepare pinned robot assets for source development.")
    app.command(check_architecture, help="Check Rust production dependency boundaries.")
    app.command(
        check_release_versions, help="Verify package versions and the release tag."
    )
    app.command(
        contracts,
        help="Generate schemas and SDKs; default to the SDK components in this checkout.",
    )
    app.command(validate, help="Validate RUA source components by lane.")
    module_name = "rua_dev.workspace_commands"
    if importlib.util.find_spec(module_name) is not None:
        importlib.import_module(module_name).register_commands(app, package_app)
    else:
        package_app.command(
            package_rua,
            name="rua",
            help="Build the hebbian-rua release wheel; never publish it.",
        )
    return app


def main() -> None:
    try:
        repository_directory = next(
            directory
            for directory in (Path.cwd(), *Path.cwd().parents)
            if (directory / "Cargo.toml").is_file() and (directory / "schemas").is_dir()
        )
    except StopIteration:
        raise SystemExit("Run rua-dev inside a RUA source checkout") from None
    os.chdir(repository_directory)
    try:
        create_app()()
    except subprocess.CalledProcessError as error:
        if error.stderr:
            stderr = (
                error.stderr.decode(errors="replace")
                if isinstance(error.stderr, bytes)
                else error.stderr
            )
            print(stderr, file=sys.stderr, end="" if stderr.endswith("\n") else "\n")
        print(f"Command failed ({error.returncode}): {error.cmd}", file=sys.stderr)
        raise SystemExit(error.returncode) from None
    except (OSError, ValueError, RuntimeError) as error:
        raise SystemExit(str(error)) from None
