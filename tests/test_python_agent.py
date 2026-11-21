import asyncio
from pathlib import Path

import pytest

from examples.python.agent_quickstart import (
    AgentRobotTools,
    Provider,
    resolve_model,
    resolve_provider,
)
from hebbian_rua import RobotToolCall, StationClient
from tests.fixture_station import ARM, running_station


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
