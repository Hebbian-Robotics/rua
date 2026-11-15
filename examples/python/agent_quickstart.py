"""Build a robot-use agent with OpenAI, Anthropic or Gemini.

Set the provider's API key, start a station, then run with --provider and --model.
The adapters translate tools, camera frames and results. You own the model loop.
"""

import argparse
import asyncio
import json
from typing import Literal

from hebbian_rua import RobotTools, StationClient
from hebbian_rua.client.contract.station_api import ArmMode
from hebbian_rua.client.environment import load_environment
from hebbian_rua.client.station_client import DEFAULT_MEDIA_SERVER, DEFAULT_SERVER

type Provider = Literal["openai", "anthropic", "gemini"]


async def run_openai(
    robot: RobotTools, task: str, model: str, instructions: str
) -> None:
    from openai import AsyncOpenAI
    from openai.types.responses import ResponseInputItemParam

    from hebbian_rua.openai import OpenAIRobotTools

    robot_tools = OpenAIRobotTools(robot)
    messages: list[ResponseInputItemParam] = [{"role": "user", "content": task}]
    async with AsyncOpenAI() as client:
        while True:
            messages.append(await robot_tools.observe())
            response = await client.responses.create(
                model=model,
                instructions=instructions,
                tools=robot_tools.tools,
                input=messages,
            )
            turn = await robot_tools.execute(response)
            messages.extend(turn.messages)
            if turn.should_stop or not turn.results:
                print(response.output_text)
                report_stop(turn.should_stop)
                break


async def run_anthropic(
    robot: RobotTools, task: str, model: str, instructions: str
) -> None:
    from anthropic import AsyncAnthropic
    from anthropic.types import MessageParam

    from hebbian_rua.anthropic import AnthropicRobotTools

    robot_tools = AnthropicRobotTools(robot)
    messages: list[MessageParam] = [{"role": "user", "content": task}]
    async with AsyncAnthropic() as client:
        while True:
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
            if turn.should_stop or not turn.results:
                print(
                    "\n".join(
                        block.text for block in response.content if block.type == "text"
                    )
                )
                report_stop(turn.should_stop)
                break


async def run_gemini(
    robot: RobotTools, task: str, model: str, instructions: str
) -> None:
    from google import genai
    from google.genai import types

    from hebbian_rua.gemini import GeminiRobotTools

    robot_tools = GeminiRobotTools(robot)
    messages = [types.Content(role="user", parts=[types.Part.from_text(text=task)])]
    configuration = types.GenerateContentConfig(
        system_instruction=instructions,
        tools=robot_tools.tools,
        automatic_function_calling=types.AutomaticFunctionCallingConfig(disable=True),
    )
    async with genai.Client().aio as client:
        while True:
            messages.append(await robot_tools.observe())
            response = await client.models.generate_content(
                model=model,
                config=configuration,
                contents=messages,
            )
            turn = await robot_tools.execute(response)
            messages.extend(turn.messages)
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
        robot = RobotTools(station, lease, arm_name, cameras)
        instructions = (
            "Use the robot tools to carry out the task. For preview_pose, provide "
            "a poses list even for one candidate; give each candidate a unique UUID v4. "
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task")
    parser.add_argument(
        "--provider", choices=["openai", "anthropic", "gemini"], default="openai"
    )
    parser.add_argument("--model", required=True, help="Model ID from your provider")
    parser.add_argument("--arm", default="follower")
    parser.add_argument("--camera", action="append", default=[])
    parser.add_argument("--server", default=DEFAULT_SERVER)
    parser.add_argument("--media-server", default=DEFAULT_MEDIA_SERVER)
    arguments = parser.parse_args()
    load_environment()

    async def main() -> None:
        async with StationClient(
            arguments.server, media_server=arguments.media_server
        ) as station:
            await run_task(
                station,
                arguments.provider,
                arguments.model,
                arguments.task,
                arguments.arm,
                arguments.camera,
            )

    asyncio.run(main())
