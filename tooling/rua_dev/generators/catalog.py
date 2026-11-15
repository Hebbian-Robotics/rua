"""Generate SDK catalog modules from the Rust-owned JSON catalog."""

import json
from pathlib import Path

from rua_dev.models import SdkTarget

PYTHON_PATH = Path(
    "hebbian_robot_use_client/hebbian_robot_use_client/llm/generated_robot_tools.py"
)
TYPESCRIPT_PATH = Path("typescript-client/src/generated/robot-tools.ts")


def generate(catalog: list[dict], context: dict, targets: frozenset[SdkTarget]) -> None:
    encoded_catalog = json.dumps(catalog, indent=2, ensure_ascii=False)
    encoded_context = json.dumps(context, indent=2, ensure_ascii=False)
    if SdkTarget.PYTHON in targets:
        PYTHON_PATH.write_text(
            '"""Generated from schemas/robot-tools.json; do not edit."""\n\n'
            "import json\n\n"
            "ROBOT_TOOL_CATALOG = json.loads(r'''\n" + encoded_catalog + "\n''')\n"
        )
        with PYTHON_PATH.open("a") as output:
            output.write(
                "\nROBOT_TOOL_CONTEXT = json.loads(r'''\n"
                + encoded_context
                + "\n''')\n"
            )

    if SdkTarget.TYPESCRIPT in targets:
        TYPESCRIPT_PATH.write_text(
            "// Generated from schemas/robot-tools.json and robot-context.json; do not edit.\nexport const robotToolSpecifications = "
            + encoded_catalog
            + ";\nexport const robotToolContext = "
            + encoded_context
            + ";\n"
        )
