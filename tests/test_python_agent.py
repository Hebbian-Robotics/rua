import asyncio

import pytest

from examples.python.agent_quickstart import AgentRobotTools
from hebbian_rua import RobotToolCall, StationClient
from tests.fixture_station import ARM, running_station


@pytest.mark.server
def test_finish_reports_completion_and_rejects_combined_robot_actions() -> None:
    with running_station() as station:

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
