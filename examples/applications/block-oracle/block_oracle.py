"""A yes/no oracle for blocks on the table.

Arrange some blocks, then type a yes/no question. The model drives the arm to
inspect the scene with the wrist camera, decides, and the arm nods (yes) or
shakes its head (no) with fixed gestures. The arm rehomes and disables between
questions, so start it resting on the table.

    uv run --project examples/python examples/applications/block-oracle/block_oracle.py --model gpt-6-astra
"""

import argparse
import asyncio
import base64
import json
import re
import time
from collections.abc import Awaitable, Mapping
from pathlib import Path
from typing import Any

from hebbian_robot_use_client.contract.station_api import ArmMode
from hebbian_robot_use_client.environment import load_environment
from hebbian_robot_use_client.station_client import Lease
from hebbian_robot_use_client.station_types import CommandResult
from openai import AsyncOpenAI

from hebbian_rua import RobotTools, StationClient
from hebbian_rua.openai import OpenAIRobotTools

ARM = "follower"
CAMERA = "table"
MAX_TURNS = 20
GESTURE_SPEED = 0.8  # rad/s; the profile caps all motion anyway
# Gripper 22 cm forward, 14 cm up, tilted ~30 degrees below horizontal, facing the
# user. Chosen so the nod keeps 0.77 rad from the wrist limit on this arm.
PRESENT_JOINTS = [0.0, -1.341, 0.926, 0.955, 0.049]
NOD_RADIANS = 0.3  # wrist_flex swing
SHAKE_RADIANS = 0.35  # shoulder_pan swing
RUNS = Path(__file__).with_name("runs")  # frames and moves per question
ANSWER = re.compile(r"ANSWER:\s*(YES|NO)\b", re.IGNORECASE)

DEFAULT_WORKSPACE = "the bluish-green Seeed Studio mat directly in front of the arm"

INSTRUCTIONS = """You are a yes/no oracle. The user has arranged blocks on the
workspace and asked a question about them. The workspace is {workspace}. Every
question is only about objects resting on the workspace: ignore everything else the
camera sees, such as the desk beyond the workspace, the floor, furniture, devices,
cables and the robot itself. An object counts only if it rests on the workspace.

Inspect before answering: move the arm so the wrist camera covers the whole
workspace, for example lift to about 15-20 cm above it and look from the left, centre
and right of the reachable area. Find the workspace edges in the images and look
closer wherever you are unsure. From above, blocks stacked on each other look like
a single block, so also look at every block from a low, side-on angle (camera about
5-8 cm above the table, pointing roughly horizontally) to judge heights and stacking
before counting. Rules: pass max_joint_speed of 0.6 or less on every
move, keep the gripper at least 5 cm above the table, never touch or move the blocks,
and do not use the gripper.

When you are confident, stop calling tools. Reply with one sentence stating how many
blocks are on the workspace, where, and which are stacked, then a final line that is exactly
'ANSWER: YES' or 'ANSWER: NO'. Robot context: """


async def inspect_and_decide(
    client: AsyncOpenAI, robot: RobotTools, model: str, workspace: str, question: str
) -> tuple[str | None, str]:
    """Let the model drive the arm until it answers; return (YES/NO or None, text)."""
    robot_tools = OpenAIRobotTools(robot)
    instructions = INSTRUCTIONS.format(workspace=workspace) + json.dumps(
        await robot.context()
    )
    messages: list = [{"role": "user", "content": question}]
    run = RUNS / time.strftime("%Y%m%d-%H%M%S")
    run.mkdir(parents=True)
    (run / "question.txt").write_text(question + "\n")
    for number in range(MAX_TURNS):
        observation = await robot_tools.observe()
        messages.append(observation)
        save_frames(observation, run / f"turn{number:02d}")
        response = await client.responses.create(
            model=model,
            instructions=instructions,
            tools=robot_tools.tools,
            input=messages,
        )
        with (run / "log.txt").open("a") as log:
            for output in response.output:
                if output.type == "function_call":
                    log.write(f"turn{number:02d} {output.name} {output.arguments}\n")
            log.write(f"turn{number:02d} text: {response.output_text}\n")
        turn = await robot_tools.execute(response)
        messages.extend(turn.messages)
        if turn.should_stop:
            return None, "Robot execution stopped after a fault or uncertain outcome."
        if not turn.results:
            match = ANSWER.search(response.output_text)
            return (match.group(1).upper() if match else None), response.output_text
    return None, f"No answer after {MAX_TURNS} turns."


def save_frames(observation: Mapping[str, Any], prefix: Path) -> None:
    """Write the camera images the model is about to see, for replaying a run."""
    images = [
        item["image_url"]
        for item in observation["content"]
        if item["type"] == "input_image"
    ]
    for index, url in enumerate(images):
        jpeg = base64.b64decode(url.split(",", 1)[1])
        prefix.with_name(f"{prefix.name}-{index}.jpg").write_bytes(jpeg)


async def gesture(lease: Lease, answer: str) -> None:
    """Move to the presenting pose, then nod (YES) or shake (NO) three times."""
    moving = lease.arm(ARM)
    joint, swing = (3, NOD_RADIANS) if answer == "YES" else (0, SHAKE_RADIANS)
    poses = [PRESENT_JOINTS]
    for _ in range(3):
        for offset in (swing, -swing):
            pose = list(PRESENT_JOINTS)
            pose[joint] += offset
            poses.append(pose)
    for pose in [*poses, PRESENT_JOINTS]:
        await expect(moving.move_to_joints(pose, max_joint_speed=GESTURE_SPEED))


async def expect(command: Awaitable[CommandResult]) -> None:
    result = await command
    if not result.completed:
        raise SystemExit(f"Move did not complete: {result.outcome} {result.detail}")


async def ask(
    station: StationClient,
    client: AsyncOpenAI,
    model: str,
    workspace: str,
    question: str,
) -> None:
    arm = station.arm(ARM)
    async with station.holding_lease(question, required_cameras=[CAMERA]) as lease:
        if (await arm.state()).mode != ArmMode.holding:
            await expect(arm.enable())  # the rest pose becomes the rehome target
        try:
            robot = RobotTools(station, lease, ARM, [CAMERA])
            answer, text = await inspect_and_decide(
                client, robot, model, workspace, question
            )
            print(f"\n{text.strip()}\n")
            if answer:
                await gesture(lease, answer)
        finally:
            await expect(lease.arm(ARM).rehome())
            await expect(arm.disable())  # the rest pose is supported by the table


async def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, help="OpenAI model ID")
    parser.add_argument(
        "--workspace",
        default=DEFAULT_WORKSPACE,
        help="Where questions apply; objects elsewhere are ignored",
    )
    arguments = parser.parse_args()
    load_environment()
    async with StationClient() as station, AsyncOpenAI() as client:
        while True:
            question = input("Ask a yes/no question about the blocks (blank to quit): ")
            if not question.strip():
                break
            await ask(station, client, arguments.model, arguments.workspace, question)


if __name__ == "__main__":
    asyncio.run(main())
