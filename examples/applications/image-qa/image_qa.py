"""Image Q&A: yes/no questions about whatever is on a display in front of the arm.

The arm rises from rest to face the display and holds there. Show an image on the
display and type a yes/no question. The model looks through the wrist camera,
searches the web when it needs facts, and the arm nods (yes) or shakes its head
(no). Leaving the session rehomes the arm and turns torque off, so start it resting
on the table.

    uv run --project examples/python examples/applications/image-qa/image_qa.py --model gpt-6-astra
"""

import argparse
import asyncio
import base64
import re
import time
from collections.abc import Awaitable
from pathlib import Path

from hebbian_robot_use_client.environment import load_environment
from hebbian_robot_use_client.station_client import Lease
from hebbian_robot_use_client.station_types import CommandResult
from openai import AsyncOpenAI

from hebbian_rua import StationClient

ARM = "follower"
CAMERA = "table"
RISE_SPEED = 0.5  # rad/s from rest to the viewing pose
GESTURE_SPEED = 0.8  # rad/s; the profile caps all motion anyway
EXPOSURE_SETTLE_SECONDS = 2.0
# Gripper 18 cm forward, 31 cm up, camera tilted ~17 degrees above horizontal so the
# display sits above the jaws. Tuned on our arm and desk; check the view on yours.
VIEW_JOINTS = [0.0, -1.440, 0.364, 0.776, 0.12]
NOD_RADIANS = 0.3  # wrist_flex swing
SHAKE_RADIANS = 0.35  # shoulder_pan swing
RUNS = Path(__file__).with_name("runs")  # frame, question and answer per question
ANSWER = re.compile(r"ANSWER:\s*(YES|NO)\b", re.IGNORECASE)

INSTRUCTIONS = """You are a yes/no oracle. The attached photo is from a robot's
wrist camera pointed at a display. The user is asking about the image shown on the
display: ignore the display's frame, the desk, the room and the robot. The photo
may be slightly blurred, angled or show screen glare; read it as well as you can.

Work it out like a careful researcher: describe what the display shows and identify
people, places or objects. Never answer from memory alone: always validate both
your identification and the facts the question depends on with web search, and for
anything that changes over time (whether someone is alive, a current record or
title) search for the latest information. Then reply with two or
three sentences giving what you see and your reasoning, then a final line that is
exactly 'ANSWER: YES' or 'ANSWER: NO'."""


async def decide(
    client: AsyncOpenAI, model: str, jpeg: bytes, question: str
) -> tuple[str | None, str]:
    """Ask the model about one frame; return (YES/NO or None, its reply)."""
    image = f"data:image/jpeg;base64,{base64.standard_b64encode(jpeg).decode()}"
    response = await client.responses.create(
        model=model,
        instructions=INSTRUCTIONS,
        tools=[{"type": "web_search"}],
        tool_choice="required",  # at least one search: no answers from memory alone
        input=[
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": question},
                    {"type": "input_image", "image_url": image, "detail": "high"},
                ],
            }
        ],
    )
    searches = sum(output.type == "web_search_call" for output in response.output)
    text = f"{response.output_text}\n(web searches: {searches})"
    match = ANSWER.search(response.output_text)
    return (match.group(1).upper() if match else None), text


async def gesture(lease: Lease, answer: str) -> None:
    """Nod (YES) or shake (NO) three times about the viewing pose."""
    moving = lease.arm(ARM)
    joint, swing = (3, NOD_RADIANS) if answer == "YES" else (0, SHAKE_RADIANS)
    for _ in range(3):
        for offset in (swing, -swing):
            pose = list(VIEW_JOINTS)
            pose[joint] += offset
            await expect(moving.move_to_joints(pose, max_joint_speed=GESTURE_SPEED))
    await expect(moving.move_to_joints(VIEW_JOINTS, max_joint_speed=GESTURE_SPEED))


async def expect(command: Awaitable[CommandResult]) -> None:
    result = await command
    if not result.completed:
        raise SystemExit(f"Move did not complete: {result.outcome} {result.detail}")


async def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, help="OpenAI model ID")
    arguments = parser.parse_args()
    load_environment()
    async with (
        StationClient() as station,
        AsyncOpenAI() as client,
        station.holding_lease("image q&a", required_cameras=[CAMERA]) as lease,
    ):
        arm = station.arm(ARM)
        await expect(arm.enable())  # the rest pose becomes the rehome target
        try:
            moving = lease.arm(ARM)
            await expect(moving.move_to_joints(VIEW_JOINTS, max_joint_speed=RISE_SPEED))
            while True:
                # A thread keeps the event loop free to renew the lease while we wait.
                question = await asyncio.to_thread(
                    input, "Ask a yes/no question about the display (blank to quit): "
                )
                if not question.strip():
                    break
                # Auto-exposure lags after a move; the first frames come out washed out.
                await asyncio.sleep(EXPOSURE_SETTLE_SECONDS)
                jpeg = (await station.camera_jpeg(CAMERA)).jpeg
                answer, text = await decide(client, arguments.model, jpeg, question)
                run = RUNS / time.strftime("%Y%m%d-%H%M%S")
                run.mkdir(parents=True)
                (run / "frame.jpg").write_bytes(jpeg)
                (run / "log.txt").write_text(f"Q: {question}\n\n{text}\n")
                print(f"\n{text.strip()}\n")
                if answer:
                    await gesture(lease, answer)
        finally:
            await expect(lease.arm(ARM).rehome())
            await expect(arm.disable())  # the rest pose is supported by the table


if __name__ == "__main__":
    asyncio.run(main())
