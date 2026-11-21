import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

import jsonref
from pydantic import TypeAdapter

from hebbian_rua.client.contract.camera_api import LiveRun, RunUsage
from rua_dev.generated import check_generated_files
from rua_dev.typescript_schemas import generate_typescript_schema


@dataclass(frozen=True)
class ViewerSchema:
    name: str
    variable_name: str
    type_name: str
    adapter: TypeAdapter

    @property
    def schema_path(self) -> Path:
        return Path("schemas") / f"{self.name}.schema.json"

    @property
    def typescript_path(self) -> Path:
        return (
            Path("examples/typescript/viewer/src/generated") / f"{self.name}-schema.ts"
        )


VIEWER_SCHEMAS = (
    ViewerSchema("live-run", "liveRunSchema", "LiveRun", TypeAdapter(LiveRun)),
    ViewerSchema("run-usage", "runUsageSchema", "RunUsage", TypeAdapter(RunUsage)),
)


def schema_text(adapter: TypeAdapter) -> str:
    schema = adapter.json_schema(mode="serialization")
    inlined_schema = jsonref.replace_refs(schema, proxies=False)
    inlined_schema.pop("$defs", None)
    return json.dumps(inlined_schema, indent=2) + "\n"


def generate_viewer_schemas(*, check: bool = False) -> None:
    for schema in VIEWER_SCHEMAS:
        schema.schema_path.parent.mkdir(parents=True, exist_ok=True)
        schema.schema_path.write_text(schema_text(schema.adapter))
        generate_typescript_schema(
            schema.schema_path,
            schema.typescript_path,
            schema.variable_name,
            schema.type_name,
        )
    subprocess.run(
        [
            "pnpm",
            "exec",
            "biome",
            "check",
            "--write",
            *[str(schema.typescript_path) for schema in VIEWER_SCHEMAS],
        ],
        check=True,
    )
    if check:
        check_generated_files(
            [
                path
                for schema in VIEWER_SCHEMAS
                for path in (schema.schema_path, schema.typescript_path)
            ]
        )
