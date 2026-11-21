import json
import os
import subprocess
from pathlib import Path

from rua_dev.generated import check_generated_files
from rua_dev.generators import catalog, commands, routes
from rua_dev.models import SdkTarget
from rua_dev.native_build import build_server
from rua_dev.robot_assets import prepare_robot_assets

SIM_SCHEMA_PATH = Path("schemas/sim-plant.schema.json")
OPENAPI_PATH = Path("schemas/openapi.json")
CATALOG_PATH = Path("schemas/robot-tools.json")
CONTEXT_PATH = Path("schemas/robot-context.json")
SIM_MODELS_PATH = Path("hebbian_rua/simulator/contract.py")
PYTHON_MODELS_PATH = Path("hebbian_rua/client/contract/station_api.py")
TYPESCRIPT_MODELS_PATH = Path("typescript-client/src/generated/api-schemas.ts")
SCHEMA_PATHS = (SIM_SCHEMA_PATH, OPENAPI_PATH, CATALOG_PATH, CONTEXT_PATH)
SDK_PATHS = {
    SdkTarget.PYTHON: (
        catalog.PYTHON_PATH,
        commands.PYTHON_PATH,
        routes.PYTHON_PATH,
    ),
    SdkTarget.RUST: (commands.RUST_PATH, routes.RUST_PATH),
    SdkTarget.TYPESCRIPT: (
        TYPESCRIPT_MODELS_PATH,
        catalog.TYPESCRIPT_PATH,
        commands.TYPESCRIPT_PATH,
        routes.TYPESCRIPT_PATH,
    ),
}


def available_sdk_targets() -> frozenset[SdkTarget]:
    directories = {
        SdkTarget.PYTHON: Path("hebbian_rua/client"),
        SdkTarget.RUST: Path("sdk"),
        SdkTarget.TYPESCRIPT: Path("typescript-client"),
    }
    return frozenset(
        target for target, directory in directories.items() if directory.is_dir()
    )


def generate_contracts(targets: frozenset[SdkTarget], *, check: bool = False) -> None:
    unavailable_targets = targets - available_sdk_targets()
    if not targets or unavailable_targets:
        raise ValueError(
            "Select SDK components present in this checkout; unavailable: "
            + ", ".join(sorted(unavailable_targets))
        )
    prepare_robot_assets(Path.cwd())
    server_binary = os.environ.get("HEBBIAN_ROBOT_USE_SERVER")
    if server_binary is None:
        server_binary = str(build_server())
    for schema_argument, destination in (
        ("--sim-plant", SIM_SCHEMA_PATH),
        ("--openapi", OPENAPI_PATH),
    ):
        result = subprocess.run(
            [server_binary, "schema", schema_argument],
            capture_output=True,
            text=True,
            check=True,
        )
        Path(destination).write_text(result.stdout)
    for arguments, destination in (
        ([], CATALOG_PATH),
        (["--", "--context"], CONTEXT_PATH),
    ):
        result = subprocess.run(
            [
                "cargo",
                "run",
                "--quiet",
                "--package",
                "hebbian-rua-contracts",
                "--example",
                "robot_tool_catalog",
                *arguments,
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        Path(destination).write_text(result.stdout)

    openapi_document = json.loads(OPENAPI_PATH.read_text())
    catalog.generate(
        json.loads(CATALOG_PATH.read_text()),
        json.loads(CONTEXT_PATH.read_text()),
        targets,
    )
    commands.generate(openapi_document, targets)
    routes.generate(openapi_document, targets)

    if SdkTarget.PYTHON in targets:
        generate_python_models()
    if SdkTarget.RUST in targets:
        subprocess.run(
            [
                "rustfmt",
                "--edition",
                "2024",
                str(commands.RUST_PATH),
                str(routes.RUST_PATH),
            ],
            check=True,
        )
    if SdkTarget.TYPESCRIPT in targets:
        generate_typescript_models()

    generated_paths = [
        *SCHEMA_PATHS,
        *[
            path
            for target in SdkTarget
            if target in targets
            for path in SDK_PATHS[target]
        ],
    ]
    if SdkTarget.PYTHON in targets:
        generated_paths.extend(output for _, _, output in python_model_schemas())
    if check:
        check_generated_files(generated_paths)


def python_model_schemas() -> list[tuple[Path, str, Path]]:
    schemas = [(OPENAPI_PATH, "openapi", PYTHON_MODELS_PATH)]
    if SIM_MODELS_PATH.parent.is_dir():
        schemas.append((SIM_SCHEMA_PATH, "jsonschema", SIM_MODELS_PATH))
    return schemas


def generate_python_models() -> None:
    for schema_path, input_format, output_path in python_model_schemas():
        subprocess.run(
            [
                "datamodel-codegen",
                "--input",
                str(schema_path),
                "--input-file-type",
                input_format,
                "--output",
                str(output_path),
                "--output-model-type",
                "pydantic_v2.BaseModel",
                "--strict-types",
                "float",
                "--formatters",
                "ruff-format",
                "ruff-check",
                "--extra-fields",
                "forbid",
                "--enable-faux-immutability",
                "--use-annotated",
                "--field-constraints",
                "--use-standard-collections",
                "--use-union-operator",
                "--target-python-version",
                "3.12",
                "--use-title-as-name",
                "--collapse-root-models",
                "--use-one-literal-as-default",
                "--disable-timestamp",
                "--use-double-quotes",
                *(
                    ["--strict-nullable", "--set-default-enum-member"]
                    if input_format == "openapi"
                    else []
                ),
            ],
            check=True,
        )
    python_outputs = [
        str(path)
        for path in (catalog.PYTHON_PATH, commands.PYTHON_PATH, routes.PYTHON_PATH)
    ]
    subprocess.run(["ruff", "check", "--fix", *python_outputs], check=True)
    subprocess.run(["ruff", "format", *python_outputs], check=True)


def generate_typescript_models() -> None:
    from rua_dev.typescript_schemas import generate_typescript_schema

    generate_typescript_schema(
        OPENAPI_PATH, TYPESCRIPT_MODELS_PATH, "apiSchemas", "ApiSchemas"
    )
    subprocess.run(
        [
            "pnpm",
            "exec",
            "biome",
            "check",
            "--write",
            *[str(path) for path in SDK_PATHS[SdkTarget.TYPESCRIPT]],
        ],
        check=True,
    )
