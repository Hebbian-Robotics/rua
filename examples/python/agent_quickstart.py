import argparse
import asyncio
import json
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
from hebbian_rua.client.station_client import DEFAULT_MEDIA_SERVER, DEFAULT_SERVER

CLI_HELP = """Build a robot-use agent with OpenAI, Anthropic or Gemini.

Set the provider's API key, start a station, then run with --provider and --model.
The adapters translate tools, camera frames and results. You own the model loop.
"""

type Provider = Literal["openai", "anthropic", "gemini"]


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
    robot: AgentRobotTools, task: str, model: str, instructions: str
) -> None:
    from openai import AsyncOpenAI
    from openai.types.responses import ResponseInputItemParam

    from hebbian_rua.client.llm.openai import OpenAIRobotTools

    robot_tools = OpenAIRobotTools(robot)
    messages: list[ResponseInputItemParam] = [{"role": "user", "content": task}]
    async with AsyncOpenAI() as client:
        capture_current_view = True
        while True:
            if capture_current_view:
                messages.append(await robot_tools.observe())
            response = await client.responses.create(
                model=model,
                instructions=instructions,
                tools=robot_tools.tools,
                input=messages,
            )
            turn = await robot_tools.execute(response)
            messages.extend(turn.messages)
            capture_current_view = any(
                result.call.name != "observe" for result in turn.results
            )
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                break
            if turn.should_stop or not turn.results:
                print(response.output_text)
                report_stop(turn.should_stop)
                break


async def run_anthropic(
    robot: AgentRobotTools, task: str, model: str, instructions: str
) -> None:
    from anthropic import AsyncAnthropic
    from anthropic.types import MessageParam

    from hebbian_rua.client.llm.anthropic import AnthropicRobotTools

    robot_tools = AnthropicRobotTools(robot)
    messages: list[MessageParam] = [{"role": "user", "content": task}]
    async with AsyncAnthropic() as client:
        capture_current_view = True
        while True:
            if capture_current_view:
                messages.append(await robot_tools.observe())
            response = await client.messages.create(
                model=model,
                max_tokens=4096,
                system=instructions,
                tools=robot_tools.tools,
                messages=messages,
            )
            turn = await robot_tools.execute(response)
            messages.extend(turn.messages)
            capture_current_view = any(
                result.call.name != "observe" for result in turn.results
            )
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                break
            if turn.should_stop or not turn.results:
                print(
                    "\n".join(
                        block.text for block in response.content if block.type == "text"
                    )
                )
                report_stop(turn.should_stop)
                break


async def run_gemini(
    robot: AgentRobotTools, task: str, model: str, instructions: str
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
        capture_current_view = True
        while True:
            if capture_current_view:
                messages.append(await robot_tools.observe())
            response = await client.models.generate_content(
                model=model,
                config=configuration,
                contents=messages,
            )
            turn = await robot_tools.execute(response)
            messages.extend(turn.messages)
            capture_current_view = any(
                result.call.name != "observe" for result in turn.results
            )
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                break
            if turn.should_stop or not turn.results:
                print(response.text or "")
                report_stop(turn.should_stop)
                break


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
                await run_openai(robot, task, model, instructions)
            case "anthropic":
                await run_anthropic(robot, task, model, instructions)
            case "gemini":
                await run_gemini(robot, task, model, instructions)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=CLI_HELP)
    parser.add_argument("task")
    parser.add_argument(
        "--provider", choices=["openai", "anthropic", "gemini"], default="openai"
    )
    parser.add_argument("--model", help="Model ID from your provider")
    parser.add_argument("--arm", default="follower")
    parser.add_argument("--camera", action="append", default=[])
    parser.add_argument("--server", default=DEFAULT_SERVER)
    parser.add_argument("--media-server", default=DEFAULT_MEDIA_SERVER)
    arguments = parser.parse_args()
    model = arguments.model
    if model is None:
        if arguments.provider != "openai":
            parser.error("--model is required for Anthropic and Gemini")
        model = "gpt-6.2-astra"
    load_environment()

    async def main(model: str) -> None:
        async with StationClient(
            arguments.server, media_server=arguments.media_server
        ) as station:
            await run_task(
                station,
                arguments.provider,
                model,
                arguments.task,
                arguments.arm,
                arguments.camera,
            )

    asyncio.run(main(model))
