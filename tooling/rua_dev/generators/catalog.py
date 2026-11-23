import json
from pathlib import Path

from rua_dev.models import SdkTarget

PYTHON_PATH = Path("hebbian_rua/client/llm/generated_robot_tools.py")
TYPESCRIPT_PATH = Path("typescript-client/src/generated/robot-tools.ts")


def generate(
    catalog: list[dict],
    context: dict,
    input_units: list[dict],
    output_units: list[list],
    outcomes: dict[str, str],
    response_fields: dict[str, list[str]],
    targets: frozenset[SdkTarget],
) -> None:
    encoded_catalog = json.dumps(catalog, indent=2, ensure_ascii=False)
    encoded_context = json.dumps(context, indent=2, ensure_ascii=False)
    encoded_units = json.dumps(input_units, indent=2, ensure_ascii=False)
    encoded_output_units = json.dumps(output_units, indent=2)
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
                + "\nROBOT_TOOL_INPUT_UNITS = json.loads(r'''\n"
                + encoded_units
                + "\n''')\n"
                + "\nMODEL_OUTPUT_UNITS = json.loads(r'''\n"
                + encoded_output_units
                + "\n''')\n"
                + "\nMODEL_OUTCOMES = json.loads(r'''\n"
                + json.dumps(outcomes, indent=2)
                + "\n''')\n"
                + "\nMODEL_RESPONSE_FIELDS = json.loads(r'''\n"
                + json.dumps(response_fields, indent=2)
                + "\n''')\n"
            )

    if SdkTarget.TYPESCRIPT in targets:
        TYPESCRIPT_PATH.write_text(
            "// Generated from schemas/robot-tools.json and robot-context.json; do not edit.\nexport const robotToolSpecifications = "
            + encoded_catalog
            + ";\nexport const robotToolContext = "
            + encoded_context
            + ";\nexport const robotToolInputUnits = "
            + encoded_units
            + " as const;\nexport const modelOutputUnits = "
            + encoded_output_units
            + " as const;\nexport const modelOutcomes = "
            + json.dumps(outcomes, indent=2)
            + " as const;\nexport const modelResponseFields = "
            + json.dumps(response_fields, indent=2)
            + " as const;\n"
        )
