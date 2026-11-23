import subprocess
import tempfile
from pathlib import Path


def generate_typescript_schema(
    schema_path: Path,
    output_path: Path,
    variable_name: str,
    type_name: str,
    *,
    component: str | None = None,
) -> None:
    with tempfile.TemporaryDirectory(
        prefix="rua-typescript-schema-"
    ) as temporary_directory:
        inlined_path = Path(temporary_directory) / "inlined.json"
        inlined = subprocess.run(
            [
                "node",
                "scripts/inline-schema-refs.ts",
                str(schema_path),
                *([component] if component is not None else []),
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        inlined_path.write_text(inlined.stdout)
        result = subprocess.run(
            [
                "pnpm",
                "exec",
                "json-schema-to-zod",
                "-i",
                str(inlined_path),
                "-n",
                variable_name,
                "-t",
                type_name,
                "--noImport",
            ],
            capture_output=True,
            text=True,
            check=True,
        )
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text('import * as z from "zod";\n' + result.stdout)
