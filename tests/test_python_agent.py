import asyncio
import json
from collections.abc import Awaitable, Callable
from pathlib import Path

import pytest

from examples.python.agent_quickstart import (
    DEFAULT_MODELS,
    AgentRobotTools,
    ModelInputCapture,
    Provider,
    resolve_model,
    resolve_provider,
    run_anthropic,
    run_gemini,
    run_openai,
)
from hebbian_rua import ModelContent, RobotOutput, RobotToolCall, StationClient
from tests.fixture_station import ARM, running_station
from tests.fixture_station_http import scripted_http_station


@pytest.mark.parametrize(
    ("provider", "model_variable", "default_model"),
    [
        ("openai", "OPENAI_MODEL", "gpt-6.2-astra"),
        ("anthropic", "ANTHROPIC_MODEL", "claude-fable-5-2"),
        ("gemini", "GEMINI_MODEL", "gemini-4-neon"),
    ],
)
def test_agent_uses_the_selected_providers_model_unless_overridden(
    provider: Provider, model_variable: str, default_model: str
) -> None:
    environment = {
        "OPENAI_MODEL": "gpt-example",
        "ANTHROPIC_MODEL": "claude-example",
        "GEMINI_MODEL": "gemini-example",
    }
    assert resolve_model(provider, None, environment) == environment[model_variable]
    assert resolve_model(provider, "explicit-model", environment) == "explicit-model"
    assert resolve_model(provider, None, {}) == default_model


def test_agent_provider_priority_and_override() -> None:
    anthropic_key = {"ANTHROPIC_API_KEY": "test-anthropic"}
    gemini_and_anthropic_keys = {"GOOGLE_API_KEY": "test-gemini", **anthropic_key}
    all_keys = {"OPENAI_API_KEY": "test-openai", **gemini_and_anthropic_keys}
    assert resolve_provider(None, all_keys) == "openai"
    assert resolve_provider(None, gemini_and_anthropic_keys) == "gemini"
    assert resolve_provider(None, anthropic_key) == "anthropic"
    assert resolve_provider("anthropic", all_keys) == "anthropic"


@pytest.mark.server
def test_finish_reports_completion_and_rejects_combined_robot_actions(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("example finish") as lease,
            ):
                robot = AgentRobotTools(client, lease, ARM)
                before = await client.arm(ARM).state()
                finish = RobotToolCall("finish", "finish", {"reason": "Task done"})
                rejected = await robot.execute(
                    [
                        RobotToolCall(
                            "motion",
                            "action_sequence",
                            {"steps": [{"gripper_opening": 0.5}]},
                        ),
                        finish,
                    ]
                )
                assert all(result.is_error for result in rejected)
                assert robot.finish_reason is None
                after = await client.arm(ARM).state()
                assert after.gripper_opening == before.gripper_opening
                assert after.last_command == before.last_command

                malformed = await robot.execute(
                    [RobotToolCall("malformed", "finish", {})]
                )
                assert malformed[0].is_error
                assert robot.finish_reason is None

                completed = await robot.execute([finish])
                assert not completed[0].is_error
                assert robot.finish_reason == "Task done"
                assert completed[0].data == {"finished": True, "reason": "Task done"}

        asyncio.run(exercise())


def provider_reply(provider: Provider, first_turn: bool) -> dict[str, object]:
    match provider:
        case "openai":
            output = (
                [
                    {
                        "type": "function_call",
                        "call_id": "observe-1",
                        "name": "observe",
                        "arguments": "{}",
                    }
                ]
                if first_turn
                else []
            )
            return {
                "id": "resp_first" if first_turn else "resp_second",
                "object": "response",
                "status": "completed",
                "output": output,
            }
        case "anthropic":
            content = (
                [
                    {
                        "type": "tool_use",
                        "id": "observe-1",
                        "name": "observe",
                        "input": {},
                    }
                ]
                if first_turn
                else [{"type": "text", "text": "Done"}]
            )
            return {
                "id": "msg_first" if first_turn else "msg_second",
                "type": "message",
                "role": "assistant",
                "stop_reason": "tool_use" if first_turn else "end_turn",
                "content": content,
                "usage": {"input_tokens": 10, "output_tokens": 1},
            }
        case "gemini":
            return {
                "id": "interaction_first" if first_turn else "interaction_second",
                "status": "requires_action" if first_turn else "completed",
                "steps": [
                    {
                        "type": "function_call",
                        "id": "observe-1",
                        "name": "observe",
                        "arguments": {},
                    }
                ]
                if first_turn
                else [],
            }


@pytest.mark.parametrize(
    ("provider", "run_agent", "base_url_variable"),
    [
        ("openai", run_openai, "OPENAI_BASE_URL"),
        ("anthropic", run_anthropic, "ANTHROPIC_BASE_URL"),
        ("gemini", run_gemini, "GOOGLE_GEMINI_BASE_URL"),
    ],
)
def test_python_agent_continues_with_new_tool_results_and_observations(
    provider: Provider,
    run_agent: Callable[[AgentRobotTools, str, str, str, int], Awaitable[None]],
    base_url_variable: str,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    requests: list[str] = []

    async def respond(
        reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        try:
            headers = (await reader.readuntil(b"\r\n\r\n")).decode().splitlines()
            lengths = [
                int(line.split(":", 1)[1])
                for line in headers
                if line.lower().startswith("content-length:")
            ]
            requests.append((await reader.readexactly(lengths[0])).decode())
            payload = json.dumps(provider_reply(provider, len(requests) == 1)).encode()
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n"
                + f"Content-Length: {len(payload)}\r\n\r\n".encode()
                + payload
            )
            await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()

    def format_observation(output: RobotOutput) -> ModelContent:
        if output.kind == "observation":
            return ModelContent("initial station state", {"table": b"initial frame"})
        return ModelContent("fresh station state", {"table": b"fresh frame"})

    async def exercise() -> None:
        async with (
            asyncio.timeout(10),
            await asyncio.start_server(respond, "127.0.0.1", 0) as provider_server,
            scripted_http_station() as station,
        ):
            port = provider_server.sockets[0].getsockname()[1]
            monkeypatch.setenv(base_url_variable, f"http://127.0.0.1:{port}")
            monkeypatch.setenv(f"{provider.upper()}_API_KEY", "local-test-key")
            monkeypatch.setenv("GOOGLE_GENAI_USE_VERTEXAI", "false")
            monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
            station.grant_lease.set()
            async with (
                StationClient(station.url) as client,
                client.holding_lease("model loop test") as lease,
            ):
                robot = AgentRobotTools(
                    client, lease, "follower"
                ).with_output_formatter(format_observation)
                robot.model_inputs = ModelInputCapture(str(tmp_path))
                await run_agent(
                    robot, "initial task", DEFAULT_MODELS[provider], "instructions", 2
                )

    asyncio.run(exercise())

    first, second = requests
    assert "initial task" in first
    assert "aW5pdGlhbCBmcmFtZQ==" in first
    assert "initial task" not in second
    assert "initial station state" not in second
    assert "aW5pdGlhbCBmcmFtZQ==" not in second
    assert "ZnJlc2ggZnJhbWU=" in second

    [run_images] = tmp_path.iterdir()
    [initial, follow_up_images] = sorted(run_images.iterdir())
    assert (initial / "1.jpg").read_bytes() == b"initial frame"
    assert (follow_up_images / "1.jpg").read_bytes() == b"fresh frame"
    assert json.loads((initial / "index.json").read_text()) == {
        "turn": 1,
        "images": [{"file": "1.jpg", "camera": "table", "tool_call_id": None}],
    }
    assert json.loads((follow_up_images / "index.json").read_text()) == {
        "turn": 2,
        "images": [{"file": "1.jpg", "camera": "table", "tool_call_id": "observe-1"}],
    }
    follow_up = json.loads(second)
    match provider:
        case "openai":
            assert follow_up["previous_response_id"] == "resp_first"
            assert follow_up["input"][0] == {
                "type": "function_call_output",
                "call_id": "observe-1",
                "output": "fresh station state",
            }
        case "anthropic":
            assert follow_up["thread"] == {
                "type": "continue",
                "previous_message_id": "msg_first",
            }
            assert follow_up["messages"][0]["content"][0] == {
                "type": "tool_result",
                "tool_use_id": "observe-1",
                "content": "fresh station state",
                "is_error": False,
            }
        case "gemini":
            assert follow_up["previous_interaction_id"] == "interaction_first"
            assert follow_up["input"][0] == {
                "type": "function_result",
                "name": "observe",
                "call_id": "observe-1",
                "result": [{"type": "text", "text": "fresh station state"}],
                "is_error": False,
            }
