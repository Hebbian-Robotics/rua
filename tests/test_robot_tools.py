import asyncio
import json
from collections.abc import Iterator
from contextlib import AsyncExitStack
from pathlib import Path

import pytest

from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.tool_catalog import (
    RobotToolConfiguration,
    all_robot_tool_capabilities,
    create_robot_tool_catalog,
)
from hebbian_rua.client.llm.types import RobotToolCall, RobotToolResult, RobotToolSpec
from hebbian_rua.client.station_client import Lease, StationClient
from tests.fixture_station import ARM
from tests.fixture_station_http import scripted_http_station

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


@pytest.mark.parametrize(
    "tool",
    create_robot_tool_catalog(all_robot_tool_capabilities()),
    ids=lambda tool: tool.name,
)
def test_every_tool_schema_stays_within_what_all_providers_accept(
    tool: RobotToolSpec,
) -> None:
    properties = tool.parameters_schema["properties"]
    assert isinstance(properties, dict)
    assert "kind" not in properties
    assert schema_keywords(tool.parameters_schema) <= PROVIDER_COMPATIBLE_KEYWORDS


@pytest.fixture
def robot() -> Iterator[RobotTools]:
    runner = asyncio.Runner()
    stack = AsyncExitStack()

    async def acquire() -> tuple[StationClient, Lease]:
        station = await stack.enter_async_context(scripted_http_station())
        station.grant_lease.set()
        client = await stack.enter_async_context(StationClient(station.url))
        lease = await stack.enter_async_context(client.holding_lease("model tools"))
        return client, lease

    client, lease = runner.run(acquire())
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
    runner.run(stack.aclose())
    runner.close()


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
    assert "fly" in results[0].text
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
                    {"goals": {"arm": {"joints": [0, 0, 0, 0, 0]}}},
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


def test_calls_run_concurrently_and_results_keep_input_order(robot: RobotTools) -> None:
    async def exercise() -> None:
        from collections.abc import Awaitable, Callable

        entered: list[str] = []
        ready = asyncio.Event()

        async def wrap(
            call: RobotToolCall, run: Callable[[], Awaitable[RobotToolResult]]
        ) -> RobotToolResult:
            entered.append(call.call_id)
            if len(entered) == 3:
                ready.set()
            await ready.wait()
            return await run()

        calls = [RobotToolCall(str(index), "unknown", {}) for index in range(3)]
        results = await asyncio.wait_for(robot.execute(calls, wrap), 2)
        assert entered == ["0", "1", "2"]
        assert [result.call.call_id for result in results] == entered
        assert all(result.is_error for result in results)

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
        assert result.data == "Invalid input: Tool fly is not available"
        assert result.is_error
        assert not result.halt
        assert not result.interrupted
        assert (
            await robot.execute_one(call)
        ).text == "Invalid input: Tool fly is not available"
        assert (
            await mapped.with_output_formatter(full_formatter).execute_one(call)
        ).text == "Invalid input: Tool fly is not available"

    asyncio.run(exercise())


def test_specialized_catalog_limits_both_schema_and_parsed_actions(
    robot: RobotTools,
) -> None:
    from hebbian_rua.client.llm.tool_catalog import (
        NamedArmSequence,
        default_robot_tool_mapping,
        filter_sequence_actions,
    )

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
    fields = schema
    for field in ("properties", "steps", "items", "properties"):
        child = fields[field]
        assert isinstance(child, dict)
        fields = child
    assert "arms" not in fields
    assert "joints_degrees" not in json.dumps(fields)
    movement = fields["goal"]
    assert isinstance(movement, dict)
    movement_properties = movement["properties"]
    assert isinstance(movement_properties, dict)
    assert "position_mm" in movement_properties
    for call in (
        RobotToolCall(
            "hidden",
            "action_sequence",
            {"steps": [{"goal": {"joints_degrees": [0, 0, 0, 0, 0]}}]},
        ),
        RobotToolCall("standalone", "set_arm_gripper", {"arm": ARM, "opening": 0.5}),
    ):
        result = asyncio.run(specialized.execute_one(call))
        assert result.is_error
        assert "not available" in result.text
    named = RobotTools(
        robot.station,
        robot.lease,
        ARM,
        configuration=RobotToolConfiguration(
            sequence_actions=frozenset({"gripper"}),
            sequence_scope=NamedArmSequence(("left", "right")),
        ),
    )
    result = asyncio.run(
        named.execute_one(
            RobotToolCall(
                "unknown-arm",
                "action_sequence",
                {"steps": [{"arms": {"other": {"gripper_opening": 0.5}}}]},
            )
        )
    )
    assert result.is_error
    assert "Arm other is not available" in result.text


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        (
            "move_arms",
            {"goals": {"arm": {"joints_degrees": [True, 0, 0, 0, 0]}}},
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
    result = asyncio.run(
        robot.execute_one(RobotToolCall("boolean-input", name, arguments))
    )
    assert isinstance(result, RobotToolResult)
    assert result.is_error
    assert "Invalid input" in result.text
