import asyncio
import json
from collections.abc import Iterator
from pathlib import Path

import pytest

from hebbian_rua.client.contract.station_api import (
    LeaseGranted,
    LeaseHolder,
    LeaseStatus,
    OnInterruption,
)
from hebbian_rua.client.llm.robot_tool_rules import (
    convert_model_arguments_to_station_units,
)
from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.tool_catalog import RobotToolConfiguration
from hebbian_rua.client.llm.tools import ROBOT_TOOLS
from hebbian_rua.client.llm.types import RobotToolCall, RobotToolResult, RobotToolSpec
from hebbian_rua.client.station_client import Lease, StationClient
from tests.fixture_station import ARM

# CONSTRAINT: The JSON Schema keywords the tools use today, all accepted by the Anthropic,
# OpenAI and Gemini APIs. A new keyword fails here, so someone checks it against
# all three providers before a model ever sees it.
PROVIDER_COMPATIBLE_KEYWORDS = {
    "$defs",
    "$ref",
    "additionalProperties",
    "anyOf",
    "const",
    "default",
    "description",
    "discriminator",
    "enum",
    "exclusiveMinimum",
    "items",
    "maxItems",
    "maximum",
    "minItems",
    "minProperties",
    "minimum",
    "oneOf",
    "properties",
    "required",
    "title",
    "type",
}


def schema_keywords(schema: object) -> set[str]:
    if isinstance(schema, list):
        return set().union(*(schema_keywords(item) for item in schema))
    if not isinstance(schema, dict):
        return set()
    keywords = set(schema)
    for keyword, value in schema.items():
        if keyword in {"properties", "$defs"}:
            keywords |= set().union(*(schema_keywords(v) for v in value.values()))
        elif keyword not in {"discriminator", "default", "const", "enum"}:
            keywords |= schema_keywords(value)
    return keywords


@pytest.mark.parametrize("tool", ROBOT_TOOLS, ids=lambda tool: tool.name)
def test_every_tool_schema_stays_within_what_all_providers_accept(
    tool: RobotToolSpec,
) -> None:
    assert "kind" not in tool.parameters_schema["properties"]
    assert schema_keywords(tool.parameters_schema) <= PROVIDER_COMPATIBLE_KEYWORDS


@pytest.fixture
def robot() -> Iterator[RobotTools]:
    client = StationClient("http://127.0.0.1:9")
    lease = Lease(
        client,
        LeaseGranted(
            lease_seconds=15,
            lease_token="unused",
            status=LeaseStatus(
                expires_in_seconds=15,
                holder=LeaseHolder.agent,
                label="test",
                on_interruption=OnInterruption.wait_for_approval,
                required_cameras=[],
            ),
        ),
    )
    yield RobotTools(
        client,
        lease,
        ARM,
        configuration=RobotToolConfiguration(
            tools=(
                "action_sequence",
                "preview_pose",
                "move_arms",
                "set_arm_gripper",
                "rehome",
                "stop",
                "observe",
            )
        ),
    )
    asyncio.run(client.close())


def test_mistakes_come_back_as_errors_the_model_can_fix(robot: RobotTools) -> None:
    results = asyncio.run(
        robot.execute(
            [
                RobotToolCall("unknown", "fly", {}),
                RobotToolCall(
                    "invalid",
                    "action_sequence",
                    {
                        "steps": [
                            {"goal": {"kind": "pose", "position_mm": [200, 0, 50]}}
                        ]
                    },
                ),
                RobotToolCall(
                    "invalid-opening",
                    "set_arm_gripper",
                    {"arm": ARM, "opening": "half"},
                ),
            ]
        )
    )
    assert all(result.is_error for result in results)
    assert "Unknown tool" in results[0].text
    assert "orientation" in results[1].text
    assert "opening" in results[2].text
    ambiguous_units = asyncio.run(
        robot.execute(
            [
                RobotToolCall(
                    "old-position",
                    "preview_pose",
                    {"poses": [{"pose": {"position": [0.2, 0, 0.05]}}]},
                ),
                RobotToolCall(
                    "old-joints",
                    "move_arms",
                    {"targets": {"arm": {"joints": [0, 0, 0, 0, 0]}}},
                ),
                RobotToolCall(
                    "old-speed",
                    "set_arm_gripper",
                    {"arm": "arm", "opening": 0.5, "max_joint_speed": 1},
                ),
            ]
        )
    )
    assert all(
        result.is_error and "explicit model units" in result.text
        for result in ambiguous_units
    )
    kind = asyncio.run(robot.execute([RobotToolCall("kind", "stop", {"kind": "stop"})]))
    assert kind[0].is_error
    assert "kind" in kind[0].text


def test_custom_description_is_scoped_to_one_model_tool_view(robot: RobotTools) -> None:
    customized = RobotTools(
        robot.station,
        robot.lease,
        ARM,
        descriptions={"set_arm_gripper": "Close the inspection gripper"},
        configuration=robot.configuration,
    )
    assert {tool.name: tool.description for tool in customized.tools} == {
        **{tool.name: tool.description for tool in robot.tools},
        "set_arm_gripper": "Close the inspection gripper",
    }
    assert "Close the inspection gripper" not in {
        tool.description for tool in robot.tools
    }


def test_model_response_uses_readable_units_and_preserves_copyable_orientation() -> (
    None
):
    from hebbian_rua import model_response

    fixtures = json.loads(
        (Path(__file__).parent / "fixtures/model-response.json").read_text()
    )
    for fixture in fixtures:
        assert model_response(fixture["input"]) == fixture["expected"]


def test_concise_views_match_the_shared_examples() -> None:
    from hebbian_rua import (
        concise_output,
        concise_response,
    )

    fixtures = json.loads(
        (Path(__file__).parent / "fixtures/model-concise.json").read_text()
    )
    for fixture in fixtures:
        keep_station_context = fixture["keep_station_context"]
        assert (
            concise_output(fixture["input"], keep_station_context=keep_station_context)
            == fixture["concise"]
        ), fixture["name"]
        assert (
            concise_response(
                fixture["input"], keep_station_context=keep_station_context
            )
            == fixture["expected"]
        ), fixture["name"]


def test_calls_run_concurrently_and_results_keep_input_order(
    robot: RobotTools, monkeypatch: pytest.MonkeyPatch
) -> None:
    from hebbian_rua.client.contract.station_api import (
        CommandRefused,
        RejectionReason,
        StationSetGripperCommand,
    )
    from hebbian_rua.client.station_client import CommandResult

    async def exercise() -> None:
        started: list[float] = []
        release_first = asyncio.Event()
        all_started = asyncio.Event()

        async def run_command(
            arm: str, command: object, **_options: object
        ) -> CommandResult:
            assert isinstance(command, StationSetGripperCommand)
            opening = command.opening
            started.append(opening)
            if len(started) == 3:
                all_started.set()
            if opening == 0.1:
                await release_first.wait()
            reason = (
                RejectionReason.stale_revision
                if opening == 0.2
                else RejectionReason.malformed
            )
            return CommandResult.of(CommandRefused(reason=reason, detail="refused"))

        monkeypatch.setattr(robot.station, "_run", run_command)
        calls = [
            RobotToolCall(
                str(opening), "set_arm_gripper", {"arm": ARM, "opening": opening}
            )
            for opening in (0.1, 0.2, 0.3)
        ]
        pending = asyncio.create_task(robot.execute(calls))
        await all_started.wait()
        assert started == [0.1, 0.2, 0.3]
        release_first.set()
        results = await pending
        assert [result.call.call_id for result in results] == ["0.1", "0.2", "0.3"]
        assert results[1].interrupted
        assert results[2].is_error

    asyncio.run(exercise())


def test_output_mapping_preserves_original_error_and_leaves_source_view_unchanged(
    robot: RobotTools,
) -> None:
    from hebbian_rua import ModelContent, RobotOutput, full_formatter

    def custom_formatter(output: RobotOutput) -> ModelContent:
        assert output.kind == "result"
        assert output.call is not None
        assert output.call.name == "fly"
        return ModelContent("User-selected explanation")

    async def exercise() -> None:
        call = RobotToolCall("unknown", "fly", {})
        mapped = robot.with_output_formatter(custom_formatter).map_tools(reversed)
        result = await mapped.execute_one(call)
        assert result.text == "User-selected explanation"
        assert result.data == "Unknown tool fly"
        assert result.is_error
        assert not result.halt
        assert not result.interrupted
        assert (await robot.execute_one(call)).text == "Unknown tool fly"
        assert (
            await mapped.with_output_formatter(full_formatter).execute_one(call)
        ).text == "Unknown tool fly"

    asyncio.run(exercise())


def test_specialized_catalog_limits_both_schema_and_parsed_actions(
    robot: RobotTools,
) -> None:
    from hebbian_rua.client.llm.tool_catalog import (
        NamedArmSequence,
        default_robot_tool_mapping,
        filter_sequence_actions,
    )
    from hebbian_rua.client.llm.tools import ActionSequenceInput
    from hebbian_rua.client.llm.types import RobotToolResult

    specialized = (
        robot.map_tools(default_robot_tool_mapping)
        .map_tools(reversed)
        .map_tools(filter_sequence_actions(lambda action: action != "joints"))
    )
    assert [tool.name for tool in specialized.tools] == [
        "observe",
        "stop",
        "preview_pose",
        "action_sequence",
    ]
    original_schema = next(
        tool.parameters_schema for tool in robot.tools if tool.name == "action_sequence"
    )
    assert "joints_degrees" in json.dumps(original_schema)
    schema = next(
        tool.parameters_schema
        for tool in specialized.tools
        if tool.name == "action_sequence"
    )
    fields = schema["properties"]["steps"]["items"]["properties"]
    assert "arms" not in fields
    assert "joints_degrees" not in json.dumps(fields)
    assert "position_mm" in fields["goal"]["properties"]
    assert isinstance(
        specialized.parse(
            RobotToolCall(
                "hidden",
                "action_sequence",
                {"steps": [{"goal": {"joints_degrees": [0, 0, 0, 0, 0]}}]},
            )
        ),
        RobotToolResult,
    )
    assert isinstance(
        specialized.parse(
            RobotToolCall("standalone", "set_arm_gripper", {"arm": ARM, "opening": 0.5})
        ),
        RobotToolResult,
    )
    implicit = specialized.parse(
        RobotToolCall(
            "implicit", "action_sequence", {"steps": [{"gripper_opening": 0.5}]}
        )
    )
    assert isinstance(implicit, ActionSequenceInput)
    assert implicit.steps[0].arms[ARM].gripper_opening == 0.5
    named = RobotTools(
        robot.station,
        robot.lease,
        ARM,
        configuration=RobotToolConfiguration(
            sequence_actions=frozenset({"gripper"}),
            sequence_scope=NamedArmSequence(("left", "right")),
        ),
    )
    parsed = named.parse(
        RobotToolCall(
            "named",
            "action_sequence",
            {
                "steps": [
                    {
                        "arms": {
                            "left": {"gripper_opening": 0.2},
                            "right": {"gripper_opening": 0.8},
                        }
                    }
                ]
            },
        )
    )
    assert isinstance(parsed, ActionSequenceInput)
    assert set(parsed.steps[0].arms) == {"left", "right"}
    assert isinstance(
        named.parse(
            RobotToolCall(
                "unknown-arm",
                "action_sequence",
                {"steps": [{"arms": {"other": {"gripper_opening": 0.5}}}]},
            )
        ),
        RobotToolResult,
    )


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        (
            "move_arms",
            {"targets": {"arm": {"joints_degrees": [True, 0, 0, 0, 0]}}},
        ),
        (
            "set_arm_gripper",
            {
                "arm": "arm",
                "opening": 0.5,
                "max_joint_speed_degrees_per_second": True,
            },
        ),
        ("set_arm_gripper", {"arm": "arm", "opening": True}),
        ("action_sequence", {"steps": [{"gripper_opening": True}]}),
    ],
)
def test_model_motion_inputs_reject_booleans(
    robot: RobotTools, name: str, arguments: object
) -> None:
    result = robot.parse(RobotToolCall("boolean-input", name, arguments))
    assert isinstance(result, RobotToolResult)
    assert result.is_error
    assert "Invalid input" in result.text


def test_tool_input_units_match_the_station_contract() -> None:
    fixture = json.loads(
        (Path(__file__).parent / "fixtures/model-input-units.json").read_text()
    )
    assert (
        convert_model_arguments_to_station_units(fixture["arguments"])
        == fixture["station_command"]
    )
