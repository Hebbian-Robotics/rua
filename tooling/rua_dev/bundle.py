import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path, PurePosixPath
from zipfile import ZipFile, ZipInfo

import tomli_w
from packaging.utils import parse_wheel_filename
from wheel.wheelfile import WheelFile


def assemble_bundle(
    repository: Path, server_wheel: Path, media_wheel: Path, output: Path
) -> Path:
    source_manifest = tomllib.loads((repository / "pyproject.toml").read_text())
    project = source_manifest["project"].copy()
    if project["name"] != "hebbian-rua":
        raise ValueError("The root package must be hebbian-rua")
    version = project["version"]
    native_platforms: set[str] | None = None
    for artifact, expected_name in (
        (server_wheel, "hebbian-robot-use-server"),
        (media_wheel, "hebbian-robot-use-media"),
    ):
        artifact_name, artifact_version, _, tags = parse_wheel_filename(artifact.name)
        if artifact_name != expected_name:
            raise ValueError(f"Expected {expected_name} wheel: {artifact}")
        if str(artifact_version) != version:
            raise ValueError(f"Wheel version does not match {version}: {artifact}")
        platforms = {tag.platform for tag in tags if tag.platform != "any"}
        native_platforms = (
            platforms if native_platforms is None else native_platforms & platforms
        )
    if not native_platforms:
        raise ValueError("Server and media wheels need matching native platform tags")
    platform_tag = ".".join(sorted(native_platforms))
    public_profiles = {
        profile.relative_to(repository).as_posix()
        for profile in (repository / "profiles").glob("*.toml")
    }
    robot_asset_pin = tomllib.loads(
        (repository / "robots/so101-assets.toml").read_text()
    )
    robot_assets = {
        f"robots/so101/{filename}" for filename in robot_asset_pin["files"]
    } | {"robots/so101-assets.toml"}
    output.mkdir(parents=True, exist_ok=True)
    # WHY: Keep staging on the output filesystem so publication is one atomic rename.
    with tempfile.TemporaryDirectory(prefix=".rua-bundle-", dir=output) as temporary:
        staging = Path(temporary)
        shutil.copytree(
            repository / "hebbian_rua",
            staging / "hebbian_rua",
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        )
        shutil.copy2(
            repository / "schemas/sim-plant.schema.json",
            staging / "hebbian_rua/sim-plant.schema.json",
        )
        shutil.copy2(repository / "LICENSE", staging / "LICENSE")
        shutil.copy2(repository / "README.md", staging / "README.md")
        (staging / "pyproject.toml").write_text(
            tomli_w.dumps(
                {
                    "project": project,
                    "build-system": source_manifest["build-system"],
                    "tool": {
                        "uv": {
                            "build-backend": {
                                "module-root": "",
                                "module-name": "hebbian_rua",
                            }
                        }
                    },
                }
            )
        )
        subprocess.run(
            [
                "uv",
                "build",
                "--wheel",
                "--project",
                str(staging),
                "--out-dir",
                str(staging / "wheels"),
            ],
            check=True,
        )
        python_wheel = next((staging / "wheels").glob("*.whl"))
        destination = output / f"hebbian_rua-{version}-py3-none-{platform_tag}.whl"
        distribution = f"hebbian_rua-{version}"
        included: set[str] = set()
        staged_wheel = staging / destination.name
        with WheelFile(staged_wheel, "w") as bundle:
            for artifact in (python_wheel, server_wheel, media_wheel):
                with ZipFile(artifact) as source:
                    for entry in source.infolist():
                        if entry.is_dir() or entry.filename.endswith("/RECORD"):
                            continue
                        parts = PurePosixPath(entry.filename).parts
                        if artifact != python_wheel and parts[0].endswith(".dist-info"):
                            continue
                        target = entry.filename
                        if parts[0].endswith(".data"):
                            target = f"{distribution}.data/" + "/".join(parts[1:])
                            asset = target.split("/data/share/hebbian-robot-use/", 1)
                            if len(asset) == 2 and not include_bundle_asset(
                                asset[1], public_profiles, robot_assets
                            ):
                                continue
                        if target in included:
                            raise ValueError(
                                f"Conflicting bundled wheel file: {target}"
                            )
                        included.add(target)
                        content = source.read(entry)
                        if target.endswith(".dist-info/WHEEL"):
                            content = (
                                content.decode()
                                .replace(
                                    "Root-Is-Purelib: true", "Root-Is-Purelib: false"
                                )
                                .replace(
                                    "Tag: py3-none-any", f"Tag: py3-none-{platform_tag}"
                                )
                                .encode()
                            )
                        if target.endswith(
                            "/data/share/hebbian-robot-use/THIRD_PARTY_NOTICES.md"
                        ):
                            content = (
                                repository / "THIRD_PARTY_NOTICES.md"
                            ).read_bytes()
                        copied_entry = ZipInfo(target, entry.date_time)
                        copied_entry.external_attr = entry.external_attr
                        copied_entry.compress_type = entry.compress_type
                        bundle.writestr(copied_entry, content)
        staged_wheel.replace(destination)
    return destination


def include_bundle_asset(
    relative_path: str, public_profiles: set[str], robot_assets: set[str]
) -> bool:
    if relative_path.startswith("dist/"):
        return False
    if relative_path.startswith("robots/"):
        return relative_path in robot_assets
    if relative_path.startswith("profiles/"):
        return relative_path in public_profiles
    return True


def build_rua_bundle(
    repository: Path,
    *,
    source_directory: Path | None = None,
    server_wheel: Path | None = None,
    media_wheel: Path | None = None,
) -> Path:
    from rua_dev.models import BuildPlatform
    from rua_dev.packaging import build_packages, built_wheel

    if (server_wheel is None) != (media_wheel is None):
        raise ValueError("Supply both --server-wheel and --media-wheel, or neither")
    if server_wheel is None or media_wheel is None:
        with tempfile.TemporaryDirectory(prefix="rua-native-wheels-") as temporary:
            directory = Path(temporary)
            build_packages(
                repository,
                BuildPlatform.LOCAL,
                include_viewer=False,
                include_media=True,
                output_directory=directory,
            )
            version = tomllib.loads((repository / "pyproject.toml").read_text())[
                "project"
            ]["version"]
            server_wheel = built_wheel(directory, "hebbian_robot_use_server", version)
            media_wheel = built_wheel(directory, "hebbian_robot_use_media", version)
            return assemble_bundle(
                source_directory or repository,
                server_wheel,
                media_wheel,
                repository / "dist",
            )
    return assemble_bundle(
        source_directory or repository,
        server_wheel.resolve(),
        media_wheel.resolve(),
        repository / "dist",
    )
