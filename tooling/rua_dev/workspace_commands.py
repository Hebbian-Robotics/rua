import subprocess
import tempfile
from pathlib import Path
from typing import Annotated

from cyclopts import App, Parameter

from rua_dev.models import BuildPlatform, SdkTarget, ValidationLane


def export_public(destination: Path) -> None:
    from rua_dev.public_export import export_public as assemble_public_export

    assemble_public_export(Path.cwd(), destination)


def viewer_schemas(*, check: bool = False) -> None:
    from rua_dev.viewer_schemas import generate_viewer_schemas

    generate_viewer_schemas(check=check)


def package_rua(
    *, server_wheel: Path | None = None, media_wheel: Path | None = None
) -> None:
    from rua_dev.bundle import build_rua_bundle
    from rua_dev.public_export import export_public as assemble_public_export

    repository = Path.cwd()
    with tempfile.TemporaryDirectory(prefix="rua-release-source-") as temporary:
        source_directory = Path(temporary) / "source"
        assemble_public_export(repository, source_directory)
        print(
            build_rua_bundle(
                repository,
                source_directory=source_directory,
                server_wheel=server_wheel,
                media_wheel=media_wheel,
            )
        )


def package_server(
    *,
    platform: BuildPlatform = BuildPlatform.LOCAL,
    without_viewer: Annotated[bool, Parameter(negative=False)] = False,
    with_media: bool = False,
) -> None:
    from rua_dev.packaging import build_packages

    if with_media and platform != BuildPlatform.LOCAL:
        raise ValueError("--with-media is only supported for --platform local")
    build_packages(
        Path.cwd(),
        platform,
        include_viewer=not without_viewer,
        include_media=with_media,
    )


def package_media() -> None:
    from rua_dev.media_archive import build_ubuntu_media_archive

    print(build_ubuntu_media_archive(Path.cwd()))


def check_viewer() -> None:
    subprocess.run(["pnpm", "check"], check=True)
    subprocess.run(["pnpm", "test:viewer"], check=True)


def check_viewer_contracts() -> None:
    from rua_dev.viewer_schemas import generate_viewer_schemas

    generate_viewer_schemas(check=True)


def contracts(
    *,
    targets: Annotated[
        tuple[SdkTarget, ...], Parameter(consume_multiple=True, negative=False)
    ] = (),
    check: bool = False,
    with_viewer: bool = False,
) -> None:
    from rua_dev.contracts import available_sdk_targets, generate_contracts

    selected_targets = frozenset(targets) or available_sdk_targets()
    if with_viewer and SdkTarget.TYPESCRIPT not in selected_targets:
        raise ValueError("--with-viewer requires the typescript target")
    generate_contracts(selected_targets, check=check)
    if with_viewer:
        from rua_dev.viewer_schemas import generate_viewer_schemas

        generate_viewer_schemas(check=check)


def validate(*, lane: ValidationLane = ValidationLane.FULL) -> None:
    from rua_dev.validation import ViewerChecks
    from rua_dev.validation import validate as run_validation

    run_validation(lane, viewer=ViewerChecks(check_viewer, check_viewer_contracts))


def register_commands(app: App, package_app: App) -> None:
    app.command(
        contracts, help="Generate available SDKs, optionally including viewer schemas."
    )
    app.command(
        validate, help="Validate shared components and the viewer in this checkout."
    )
    app.command(
        export_public,
        name="export-public",
        help="Assemble the reduced source used to build the Python wheel in a new directory.",
    )
    app.command(
        viewer_schemas,
        name="viewer-schemas",
        help="Generate the viewer's Python-owned JSON and TypeScript schemas.",
    )
    package_app.command(
        package_rua,
        name="rua",
        help="Build the single public hebbian-rua wheel; never publish it.",
    )
    package_app.command(
        package_server,
        name="server",
        help="Build the server wheel, optionally including the local media wheel.",
    )
    package_app.command(
        package_media,
        name="media",
        help="Package the locally built media wheel as an Ubuntu binary archive.",
    )
