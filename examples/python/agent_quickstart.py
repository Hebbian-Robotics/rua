import argparse
import asyncio
import json
import os
from collections.abc import Mapping, Sequence
from typing import Literal

from pydantic import BaseModel, ConfigDict

from hebbian_rua import (
    RobotToolCall,
    RobotToolResult,
    RobotTools,
    RobotToolSpec,
    StationClient,
    filter_sequence_actions,
)
from hebbian_rua.client.contract.station_api import ArmMode
from hebbian_rua.client.environment import load_environment
from hebbian_rua.client.llm.robot_tools import CallWrapper

CLI_HELP = """Build a robot-use agent with OpenAI, Anthropic or Gemini.

Set a provider API key, then start a station and run the agent.
The adapters translate tools, camera frames and results. You own the model loop.
"""

type Provider = Literal["openai", "anthropic", "gemini"]

PROVIDER_PRIORITY: tuple[Provider, ...] = ("openai", "gemini", "anthropic")
DEFAULT_MODELS: Mapping[Provider, str] = {
    "openai": "gpt-6.2-astra",
    "gemini": "gemini-4-neon",
    "anthropic": "claude-fable-5-2",
}
DEFAULT_MAX_TURNS = 20


class FinishArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    reason: str


class AgentRobotTools(RobotTools):
    finish_reason: str | None = None

    @property
    def tools(self) -> list[RobotToolSpec]:
        return [
            *super().tools,
            RobotToolSpec(
                name="finish",
                description="End the run and report the task outcome. Call alone.",
                parameters_schema=FinishArguments.model_json_schema(),
            ),
        ]

    async def execute(
        self,
        calls: Sequence[RobotToolCall],
        wrap: CallWrapper | None = None,
        *,
        expected_revision: int | None = None,
        expected_revisions: Mapping[str, int] | None = None,
    ) -> list[RobotToolResult]:
        if not any(call.name == "finish" for call in calls):
            return await super().execute(
                calls,
                wrap,
                expected_revision=expected_revision,
                expected_revisions=expected_revisions,
            )
        try:
            if len(calls) != 1:
                raise ValueError("Call finish alone; this batch was not executed.")
            arguments = FinishArguments.model_validate(calls[0].arguments)
        except ValueError as error:
            return [
                RobotToolResult(call, str(error), is_error=True, data=str(error))
                for call in calls
            ]
        self.finish_reason = arguments.reason
        return [
            RobotToolResult(
                calls[0],
                arguments.reason,
                is_error=False,
                data={"finished": True, "reason": arguments.reason},
            )
        ]


async def run_openai(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from openai import AsyncOpenAI
    from openai.types.responses import ResponseInputItemParam

    from hebbian_rua.client.llm.openai import OpenAIRobotTools

    robot_tools = OpenAIRobotTools(robot)
    messages: list[ResponseInputItemParam] = [{"role": "user", "content": task}]
    async with AsyncOpenAI() as client:
        revisions: Mapping[str, int] | None = None
        for _ in range(max_turns):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                messages.append(observation.message)
                revisions = observation.revisions
            response = await client.responses.create(
                model=model,
                instructions=instructions,
                tools=robot_tools.tools,
                input=messages,
            )
            turn = await robot_tools.execute(response, expected_revisions=revisions)
            messages.extend(turn.messages)
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(response.output_text)
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


async def run_anthropic(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from anthropic import AsyncAnthropic
    from anthropic.types import MessageParam

    from hebbian_rua.client.llm.anthropic import AnthropicRobotTools

    robot_tools = AnthropicRobotTools(robot)
    messages: list[MessageParam] = [{"role": "user", "content": task}]
    async with AsyncAnthropic() as client:
        revisions: Mapping[str, int] | None = None
        for _ in range(max_turns):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                messages.append(observation.message)
                revisions = observation.revisions
            response = await client.messages.create(
                model=model,
                max_tokens=4096,
                system=instructions,
                tools=robot_tools.tools,
                messages=messages,
            )
            turn = await robot_tools.execute(response, expected_revisions=revisions)
            messages.extend(turn.messages)
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(
                    "\n".join(
                        block.text for block in response.content if block.type == "text"
                    )
                )
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


async def run_gemini(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from google import genai
    from google.genai import types

    from hebbian_rua.client.llm.gemini import GeminiRobotTools

    robot_tools = GeminiRobotTools(robot)
    messages = [types.Content(role="user", parts=[types.Part.from_text(text=task)])]
    configuration = types.GenerateContentConfig(
        system_instruction=instructions,
        tools=robot_tools.tools,
        automatic_function_calling=types.AutomaticFunctionCallingConfig(disable=True),
    )
    async with genai.Client().aio as client:
        revisions: Mapping[str, int] | None = None
        for _ in range(max_turns):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                messages.append(observation.message)
                revisions = observation.revisions
            response = await client.models.generate_content(
                model=model,
                config=configuration,
                contents=messages,
            )
            turn = await robot_tools.execute(response, expected_revisions=revisions)
            messages.extend(turn.messages)
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(response.text or "")
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


def report_stop(should_stop: bool) -> None:
    if should_stop:
        print("Robot execution stopped after a fault or an uncertain outcome.")


async def run_task(
    station: StationClient,
    provider: Provider,
    model: str,
    task: str,
    arm_name: str,
    cameras: list[str],
    max_turns: int = DEFAULT_MAX_TURNS,
) -> None:
    async with station.holding_lease(task, required_cameras=cameras) as lease:
        robot = AgentRobotTools(station, lease, arm_name, cameras).map_tools(
            filter_sequence_actions(lambda action: action != "joints")
        )
        instructions = (
            "Operate the single configured SO-101 using Cartesian targets and "
            "gripper actions. In the base frame, +X is forward from the arm, "
            "+Y is left, and +Z is up. Camera image directions depend on camera "
            "pose. Choose the next useful action; use preview for a "
            "specific feasibility question. Fresh observations follow action "
            "batches automatically. After a limited or incomplete action, read its "
            "summary before replanning: a limited target's pose_limited.advice "
            "gives the shortfall, where the arm went and an alternative to try; a "
            "stopped_short move's completion detail gives how far short it stopped "
            "and where the tool rests, usually against the table or an object "
            "(compare with surface_guard). Check the images, then aim the next "
            "target where it can be reached; do not repeat it. Call finish alone "
            "to report the task outcome and end the run. "
            "Robot context: " + json.dumps(await robot.context())
        )
        arm = station.arm(arm_name)
        if (await arm.state()).mode != ArmMode.holding:
            enabled = await arm.enable()
            if not enabled.completed:
                raise SystemExit(f"Could not enable the arm: {enabled}")
        match provider:
            case "openai":
                await run_openai(robot, task, model, instructions, max_turns)
            case "anthropic":
                await run_anthropic(robot, task, model, instructions, max_turns)
            case "gemini":
                await run_gemini(robot, task, model, instructions, max_turns)


def positive_turn_limit(value: str) -> int:
    turns = int(value)
    if turns < 1:
        raise argparse.ArgumentTypeError("expected a positive turn limit")
    return turns


def resolve_model(
    provider: Provider, explicit_model: str | None, environment: Mapping[str, str]
) -> str:
    model_variable = f"{provider.upper()}_MODEL"
    if explicit_model is not None:
        if not explicit_model.strip():
            raise ValueError("Pass a nonempty model ID with --model")
        return explicit_model
    environment_model = environment.get(model_variable)
    return (
        environment_model
        if environment_model and environment_model.strip()
        else DEFAULT_MODELS[provider]
    )


def configured_api_key(
    provider: Provider, environment: Mapping[str, str]
) -> str | None:
    primary_key = environment.get(f"{provider.upper()}_API_KEY")
    if primary_key and primary_key.strip():
        return primary_key
    google_key = environment.get("GOOGLE_API_KEY")
    return (
        google_key
        if provider == "gemini" and google_key and google_key.strip()
        else None
    )


def resolve_provider(
    explicit_provider: Provider | None, environment: Mapping[str, str]
) -> Provider:
    if explicit_provider is not None:
        return explicit_provider
    return next(
        (
            provider
            for provider in PROVIDER_PRIORITY
            if configured_api_key(provider, environment)
        ),
        PROVIDER_PRIORITY[0],
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=CLI_HELP)
    parser.add_argument("task")
    parser.add_argument(
        "--provider",
        choices=PROVIDER_PRIORITY,
        help="Default: first configured API key in order OpenAI, Gemini, Anthropic",
    )
    parser.add_argument(
        "--model",
        help="Model ID (overrides OPENAI_MODEL, ANTHROPIC_MODEL or GEMINI_MODEL)",
    )
    parser.add_argument(
        "--max-turns",
        type=positive_turn_limit,
        default=DEFAULT_MAX_TURNS,
        help=f"Maximum number of model turns (default: {DEFAULT_MAX_TURNS})",
    )
    parser.add_argument("--arm", help="Arm name. Defaults to the station's sole arm.")
    parser.add_argument("--camera", action="append", default=[])
    parser.add_argument("--server", help="Station URL (overrides HEBBIAN_STATION_URL)")
    parser.add_argument(
        "--media-server", help="Media URL (overrides HEBBIAN_MEDIA_SERVER)"
    )
    arguments = parser.parse_args()
    load_environment()
    provider = resolve_provider(arguments.provider, os.environ)
    try:
        model = resolve_model(provider, arguments.model, os.environ)
    except ValueError as error:
        parser.error(str(error))
    if configured_api_key(provider, os.environ) is None:
        parser.error(f"Set {provider.upper()}_API_KEY before running the agent")

    async def main() -> None:
        async with StationClient(
            arguments.server, media_server=arguments.media_server
        ) as station:
            try:
                arm_name = await station.resolve_arm_name(arguments.arm)
            except ValueError as error:
                parser.error(str(error))
            await run_task(
                station,
                provider,
                model,
                arguments.task,
                arm_name,
                arguments.camera,
                arguments.max_turns,
            )

    asyncio.run(main())
