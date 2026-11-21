import asyncio
import json
from typing import Literal

import pytest
from anthropic.types import Message
from google.genai import types
from openai.types.responses import Response

from hebbian_rua import (
    ModelContent,
    RobotOutput,
    RobotToolCall,
    RobotToolConfiguration,
    RobotToolResult,
    RobotTools,
    StationClient,
    default_formatter,
)
from hebbian_rua.client.llm.anthropic import AnthropicRobotTools
from hebbian_rua.client.llm.gemini import GeminiRobotTools
from hebbian_rua.client.llm.openai import OpenAIRobotTools
from hebbian_rua.client.llm.tool_turn import RobotToolTurn
from tests.fixture_station_http import scripted_http_station

type Provider = Literal["openai", "anthropic", "gemini"]


def openai_response(*, incomplete: bool = False) -> Response:
    return Response.model_validate(
        {
            "id": "response-1",
            "object": "response",
            "created_at": 0,
            "model": "test-model",
            "status": "incomplete" if incomplete else "completed",
            "parallel_tool_calls": True,
            "tool_choice": "auto",
            "tools": [],
            "output": [
                {
                    "type": "reasoning",
                    "id": "reasoning-1",
                    "summary": [{"type": "summary_text", "text": "Inspect the robot"}],
                    "encrypted_content": "reasoning-state",
                },
                {
                    "type": "function_call",
                    "call_id": "first",
                    "name": "observe",
                    "arguments": "{}",
                },
                {
                    "type": "function_call",
                    "call_id": "second",
                    "name": "observe",
                    "arguments": "{}",
                },
                {
                    "type": "function_call",
                    "call_id": "invalid",
                    "name": "set_arm_gripper",
                    "arguments": "{",
                },
            ],
        }
    )


def anthropic_response(*, incomplete: bool = False) -> Message:
    return Message.model_validate(
        {
            "id": "message-1",
            "type": "message",
            "role": "assistant",
            "model": "test-model",
            "stop_reason": "max_tokens" if incomplete else "tool_use",
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Inspect the robot",
                    "signature": "thinking-signature",
                },
                {"type": "tool_use", "id": "first", "name": "observe", "input": {}},
                {"type": "tool_use", "id": "second", "name": "observe", "input": {}},
                {
                    "type": "tool_use",
                    "id": "invalid",
                    "name": "set_arm_gripper",
                    "input": {},
                },
            ],
        }
    )


def gemini_response(*, incomplete: bool = False) -> types.GenerateContentResponse:
    return types.GenerateContentResponse(
        candidates=[
            types.Candidate(
                finish_reason=types.FinishReason.MAX_TOKENS
                if incomplete
                else types.FinishReason.STOP,
                content=types.Content(
                    role="model",
                    parts=[
                        types.Part(
                            text="Inspect the robot",
                            thought=True,
                            thought_signature=b"thought-signature",
                        ),
                        types.Part(
                            function_call=types.FunctionCall(
                                id="first", name="observe", args={}
                            )
                        ),
                        # CONSTRAINT: Older Gemini models can omit IDs, even for repeated tool names.
                        types.Part(
                            function_call=types.FunctionCall(name="observe", args={})
                        ),
                        types.Part(
                            function_call=types.FunctionCall(
                                id="invalid", name="set_arm_gripper", args={}
                            )
                        ),
                    ],
                ),
            )
        ]
    )


@pytest.mark.parametrize("provider", ["openai", "anthropic", "gemini"])
def test_provider_turn_preserves_history_correlation_and_robot_errors(
    provider: Provider,
) -> None:
    async def exercise() -> None:
        async with scripted_http_station() as station:
            station.grant_lease.set()
            async with (
                StationClient(station.url) as client,
                client.holding_lease("provider adapter test") as lease,
            ):
                selected_images = {
                    "Sequence move image 1: table, +0.250s, motion": b"first",
                    "table": b"current",
                }

                def custom_formatter(output: RobotOutput) -> ModelContent:
                    content = default_formatter(output)
                    if (
                        output.kind == "result"
                        and isinstance(output.data, dict)
                        and "arms" in output.data
                    ):
                        return ModelContent(
                            json.dumps({"arms": "selected by user"}),
                            dict(selected_images),
                        )
                    if output.kind == "observation":
                        return ModelContent(
                            content.text,
                            dict(selected_images),
                        )
                    return content

                robot = RobotTools(
                    client,
                    lease,
                    "follower",
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
                ).with_output_formatter(custom_formatter)
                match provider:
                    case "openai":
                        adapter = OpenAIRobotTools(robot)
                        observation = await adapter.observe()
                        assert observation.get("role") == "user"
                        blocks = json.loads(json.dumps(observation))["content"]
                        assert blocks[1] == {
                            "type": "input_text",
                            "text": "Camera: Sequence move image 1: table, +0.250s, motion",
                        }
                        assert (
                            blocks[2]["image_url"] == "data:image/jpeg;base64,Zmlyc3Q="
                        )
                        assert blocks[3] == {
                            "type": "input_text",
                            "text": "Camera: table",
                        }
                        assert (
                            blocks[4]["image_url"]
                            == "data:image/jpeg;base64,Y3VycmVudA=="
                        )
                        with pytest.raises(ValueError, match="incomplete"):
                            await adapter.execute(openai_response(incomplete=True))
                        turn = await adapter.execute(openai_response())
                        messages = json.loads(json.dumps(turn.messages))
                        assert messages[0]["type"] == "reasoning"
                        assert messages[0]["encrypted_content"] == "reasoning-state"
                        outputs = [
                            message
                            for message in messages
                            if message.get("type") == "function_call_output"
                        ]
                        assert [output["call_id"] for output in outputs] == [
                            "first",
                            "second",
                            "invalid",
                        ]
                        assert "Invalid input" in outputs[-1]["output"]
                        assert (
                            messages[-1]["content"][1]["text"]
                            == "Camera: Sequence move image 1: table, +0.250s, motion"
                        )
                        assert (
                            messages[-1]["content"][2]["image_url"]
                            == "data:image/jpeg;base64,Zmlyc3Q="
                        )
                        assert (
                            json.loads(outputs[0]["output"])["arms"]
                            == "selected by user"
                        )
                    case "anthropic":
                        adapter = AnthropicRobotTools(robot)
                        observation = await adapter.observe()
                        assert observation.get("role") == "user"
                        blocks = json.loads(json.dumps(observation))["content"]
                        assert blocks[1] == {
                            "type": "text",
                            "text": "Camera: Sequence move image 1: table, +0.250s, motion",
                        }
                        assert blocks[2]["source"]["data"] == "Zmlyc3Q="
                        assert blocks[3] == {"type": "text", "text": "Camera: table"}
                        assert blocks[4]["source"]["data"] == "Y3VycmVudA=="
                        with pytest.raises(ValueError, match="incomplete"):
                            await adapter.execute(anthropic_response(incomplete=True))
                        turn = await adapter.execute(anthropic_response())
                        messages = json.loads(json.dumps(turn.messages))
                        assistant_blocks = list(messages[0]["content"])
                        assert assistant_blocks[0]["signature"] == "thinking-signature"
                        outputs = list(messages[1]["content"])
                        assert [output["tool_use_id"] for output in outputs] == [
                            "first",
                            "second",
                            "invalid",
                        ]
                        assert outputs[-1]["is_error"]
                        assert (
                            messages[-1]["content"][1]["text"]
                            == "Camera: Sequence move image 1: table, +0.250s, motion"
                        )
                        assert (
                            messages[-1]["content"][2]["source"]["data"] == "Zmlyc3Q="
                        )
                        assert (
                            json.loads(outputs[0]["content"])["arms"]
                            == "selected by user"
                        )
                    case "gemini":
                        adapter = GeminiRobotTools(robot)
                        observation = await adapter.observe()
                        assert observation.role == "user"
                        assert observation.parts is not None
                        assert (
                            observation.parts[1].text
                            == "Camera: Sequence move image 1: table, +0.250s, motion"
                        )
                        assert observation.parts[2].inline_data is not None
                        assert observation.parts[2].inline_data.data == b"first"
                        assert observation.parts[3].text == "Camera: table"
                        assert observation.parts[4].inline_data is not None
                        assert observation.parts[4].inline_data.data == b"current"
                        with pytest.raises(ValueError, match="incomplete"):
                            await adapter.execute(gemini_response(incomplete=True))
                        response = gemini_response()
                        turn = await adapter.execute(response)
                        assert response.candidates
                        assert turn.messages[0] == response.candidates[0].content
                        messages = [message.model_dump() for message in turn.messages]
                        assert (
                            messages[0]["parts"][0]["thought_signature"]
                            == b"thought-signature"
                        )
                        outputs = [
                            part["function_response"] for part in messages[1]["parts"]
                        ]
                        assert [output["id"] for output in outputs] == [
                            "first",
                            None,
                            "invalid",
                        ]
                        assert outputs[0]["name"] == outputs[1]["name"] == "observe"
                        assert "error" in outputs[-1]["response"]
                        assert (
                            messages[-1]["parts"][1]["text"]
                            == "Camera: Sequence move image 1: table, +0.250s, motion"
                        )
                        assert (
                            messages[-1]["parts"][2]["inline_data"]["data"] == b"first"
                        )
                        assert (
                            outputs[0]["response"]["result"]["arms"]
                            == "selected by user"
                        )
                assert isinstance(turn.results[0].data, dict)
                assert isinstance(turn.results[0].data["arms"], list)
                assert len(turn.results) == 3
                assert [result.is_error for result in turn.results] == [
                    False,
                    False,
                    True,
                ]
                assert not turn.should_stop

    asyncio.run(asyncio.wait_for(exercise(), timeout=10))


@pytest.mark.parametrize(
    ("halt", "interrupted", "should_stop"),
    [
        (False, False, False),
        (True, False, True),
        (False, True, True),
    ],
)
def test_turn_stops_for_faults_and_uncertain_execution(
    halt: bool, interrupted: bool, should_stop: bool
) -> None:
    result = RobotToolResult(
        RobotToolCall("call", "observe", {}),
        "error",
        data="error",
        is_error=True,
        halt=halt,
        interrupted=interrupted,
    )
    turn: RobotToolTurn[str] = RobotToolTurn([], [result])
    assert turn.should_stop is should_stop
