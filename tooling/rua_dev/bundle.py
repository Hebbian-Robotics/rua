import subprocess
import tempfile
import tomllib
from pathlib import Path, PurePosixPath
from zipfile import ZipFile, ZipInfo

from packaging.utils import parse_wheel_filename
from wheel.wheelfile import WheelFile


def bundle_platforms(
    sdk_wheel: Path, server_wheel: Path, media_wheel: Path, version: str
) -> set[str]:
    native_platforms: set[str] | None = None
    for artifact, expected_name in (
        (sdk_wheel, "hebbian-rua"),
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
    return native_platforms


def assemble_bundle(
    repository: Path,
    sdk_wheel: Path,
    server_wheel: Path,
    media_wheel: Path,
    output: Path,
) -> Path:
    source_manifest = tomllib.loads((repository / "pyproject.toml").read_text())
    project = source_manifest["project"].copy()
    if project["name"] != "hebbian-rua":
        raise ValueError("The root package must be hebbian-rua")
    version = project["version"]
    native_platforms = bundle_platforms(sdk_wheel, server_wheel, media_wheel, version)
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
        _, _, _, sdk_tags = parse_wheel_filename(sdk_wheel.name)
        if {(tag.interpreter, tag.abi) for tag in sdk_tags} != {("cp312", "abi3")}:
            raise ValueError(f"Expected a CPython 3.12 stable ABI wheel: {sdk_wheel}")
        destination = output / f"hebbian_rua-{version}-cp312-abi3-{platform_tag}.whl"
        distribution = f"hebbian_rua-{version}"
        included: set[str] = set()
        staged_wheel = staging / destination.name
        with WheelFile(staged_wheel, "w") as bundle:
            for artifact in (sdk_wheel, server_wheel, media_wheel):
                with ZipFile(artifact) as source:
                    for entry in source.infolist():
                        if entry.is_dir() or entry.filename.endswith("/RECORD"):
                            continue
                        parts = PurePosixPath(entry.filename).parts
                        if artifact != sdk_wheel and parts[0].endswith(".dist-info"):
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
                            metadata = [
                                line
                                for line in content.decode().splitlines()
                                if not line.startswith("Tag:")
                            ]
                            metadata.extend(
                                f"Tag: cp312-abi3-{platform}"
                                for platform in sorted(native_platforms)
                            )
                            content = ("\n".join(metadata) + "\n").encode()
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
            bundle.write(
                str(repository / "schemas/sim-plant.schema.json"),
                "hebbian_rua/sim-plant.schema.json",
            )
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


def build_rua_bundle(repository: Path, *, source_directory: Path | None = None) -> Path:
    from rua_dev.packaging import build_component_wheels, built_wheel

    with tempfile.TemporaryDirectory(prefix="rua-native-wheels-") as temporary:
        directory = Path(temporary)
        build_component_wheels(repository, directory)
        version = tomllib.loads((repository / "pyproject.toml").read_text())["project"][
            "version"
        ]
        subprocess.run(
            [
                "uvx",
                "--from",
                "maturin>=1.12,<2",
                "maturin",
                "build",
                "--locked",
                "--profile",
                "python-extension",
                "--compatibility",
                "off",
                "--out",
                str(directory),
            ],
            cwd=repository,
            check=True,
        )
        return assemble_bundle(
            source_directory or repository,
            built_wheel(directory, "hebbian_rua", version),
            built_wheel(directory, "hebbian_robot_use_server", version),
            built_wheel(directory, "hebbian_robot_use_media", version),
            repository / "dist",
        )
