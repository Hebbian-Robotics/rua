import json
import shutil
import subprocess
from collections.abc import Iterator
from pathlib import Path

from pydantic import BaseModel, TypeAdapter


class CargoPackage(BaseModel):
    id: str
    name: str
    version: str
    manifest_path: Path
    license: str | None = None
    license_file: str | None = None


class CargoDependencyKind(BaseModel):
    kind: str | None


class CargoDependency(BaseModel):
    pkg: str
    dep_kinds: list[CargoDependencyKind]


class CargoResolveNode(BaseModel):
    id: str
    deps: list[CargoDependency]


class CargoResolve(BaseModel):
    nodes: list[CargoResolveNode]


class CargoMetadata(BaseModel):
    packages: list[CargoPackage]
    workspace_members: frozenset[str]
    resolve: CargoResolve


class LicenseProvenance(BaseModel):
    repository: str
    commit: str
    license_text_source: str | None = None


class LicenseFallback(LicenseProvenance):
    name: str
    version: str
    directory: str
    files: list[str]


class LicenseInventoryEntry(BaseModel):
    name: str
    version: str
    license: str | None
    directory: str
    files: list[str]
    upstream: LicenseProvenance | None


FALLBACK_MANIFEST = TypeAdapter(list[LicenseFallback])


def _distribution_packages(
    metadata: CargoMetadata, package_name: str
) -> Iterator[CargoPackage]:
    packages = {package.id: package for package in metadata.packages}
    nodes = {node.id: node for node in metadata.resolve.nodes}
    root = next(
        (
            package
            for package in metadata.packages
            if package.name == package_name and package.id in metadata.workspace_members
        ),
        None,
    )
    if root is None:
        raise ValueError(f"Cargo workspace has no package named {package_name}")
    pending = [root.id]
    included: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in included:
            continue
        included.add(package_id)
        package = packages.get(package_id)
        node = nodes.get(package_id)
        if package is None or node is None:
            raise ValueError(
                f"Cargo metadata is missing package or resolve node {package_id}"
            )
        yield package
        # WHY: Cargo uses null for normal edges. Include unfamiliar kinds so
        # new Cargo edge types cannot silently omit a dependency's license.
        pending.extend(
            dependency.pkg
            for dependency in node.deps
            if any(kind.kind != "dev" for kind in dependency.dep_kinds)
        )


def collect_rust_licenses(
    repository_root: Path, package_name: str, destination: Path, target_triple: str
) -> None:
    metadata_process = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            target_triple,
        ],
        cwd=repository_root,
        capture_output=True,
        text=True,
        check=True,
    )
    metadata = CargoMetadata.model_validate_json(metadata_process.stdout)
    packages = list(_distribution_packages(metadata, package_name))
    fallback_root = repository_root / "licenses" / "rust"
    fallbacks = {
        (entry.name, entry.version): entry
        for entry in FALLBACK_MANIFEST.validate_json(
            (fallback_root / "manifest.json").read_bytes()
        )
    }

    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(fallback_root / "README.md", destination / "README.md")
    manifest: list[LicenseInventoryEntry] = []
    for package in packages:
        package_key = f"{package.name}-{package.version}"
        package_directory = package.manifest_path.parent
        license_files = [
            path
            for path in package_directory.iterdir()
            if path.is_file()
            and path.name.lower().startswith(
                ("license", "licence", "copying", "notice")
            )
        ]
        declared_license_file = package.license_file
        if declared_license_file:
            license_path = package_directory / declared_license_file
            if license_path.is_file() and license_path not in license_files:
                license_files.append(license_path)
        if not license_files and package.id in metadata.workspace_members:
            license_files.append(repository_root / "LICENSE")
        fallback = fallbacks.get((package.name, package.version))
        if fallback:
            license_files.extend(
                fallback_root / fallback.directory / filename
                for filename in fallback.files
            )
        if license_files:
            package_target = destination / package_key
            package_target.mkdir(exist_ok=True)
            for source in license_files:
                shutil.copy2(source, package_target / source.name)
        manifest.append(
            LicenseInventoryEntry(
                name=package.name,
                version=package.version,
                license=package.license,
                directory=package_key,
                files=sorted({path.name for path in license_files}),
                upstream=fallback,
            )
        )
    manifest.sort(key=lambda package: (package.name, package.version))
    (destination / "manifest.json").write_text(
        json.dumps(
            [entry.model_dump(mode="json", exclude_unset=True) for entry in manifest],
            indent=2,
            ensure_ascii=False,
        )
        + "\n"
    )
    missing = [
        f"{package.name} {package.version}" for package in manifest if not package.files
    ]
    if missing:
        raise ValueError("Missing Rust dependency license files: " + ", ".join(missing))
