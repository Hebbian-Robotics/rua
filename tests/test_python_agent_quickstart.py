import asyncio
from types import SimpleNamespace

import pytest

from examples.python.agent_quickstart import (
    AgentRobotTools,
    must_stop_after_tools,
    next_observation_revisions,
)
from hebbian_rua import RobotToolCall, RobotToolResult
from hebbian_rua.client.contract.station_api import ArmMode


def test_observation_is_reused_for_read_only_turns_and_refreshed_after_motion() -> None:
    revisions = {"arm": 1}
    read_only = [
        RobotToolResult(RobotToolCall(name, name, arguments), "ok", False, data={})
        for name, arguments in [
            ("update_plan", {"steps": []}),
            ("update_notes", {"notes": []}),
            ("preview_pose", {}),
            ("observe", {"source": "last_sequence"}),
        ]
    ]
    assert next_observation_revisions(revisions, read_only) == revisions

    observed_revisions = {"arm": 2}
    observe = RobotToolResult(
        RobotToolCall("live", "observe", {"source": "live"}),
        "ok",
        False,
        data={},
        observed_revisions=observed_revisions,
    )
    move = RobotToolResult(
        RobotToolCall("motion", "action_sequence", {}),
        "ok",
        False,
        data={},
    )
    assert (
        next_observation_revisions(revisions, [*read_only, observe])
        == observed_revisions
    )
    assert next_observation_revisions(revisions, [move, observe]) is None


def test_changed_arm_revision_rejects_the_batch_before_dispatch() -> None:
    async def state() -> SimpleNamespace:
        return SimpleNamespace(mode=ArmMode.holding, revision=2)

    robot = object.__new__(AgentRobotTools)
    robot.arm = "arm"
    robot.station = SimpleNamespace(  # ty: ignore[invalid-assignment]
        arm=lambda _name: SimpleNamespace(state=state),
        lease_lost=asyncio.Event(),
    )
    call = RobotToolCall("motion", "action_sequence", {})
    [result] = asyncio.run(robot.execute([call], expected_revisions={"arm": 1}))

    assert result.is_error
    assert isinstance(result.data, dict)
    assert result.data["reason"] == "stale_revision"
    assert not must_stop_after_tools([result])
    assert next_observation_revisions({"arm": 1}, [result]) is None


@pytest.mark.parametrize(
    ("data", "interrupted", "must_stop"),
    [
        ({"status": "refused", "reason": "interlocked", "detail": "Wait"}, True, False),
        (
            {"status": "refused", "reason": "lease_held_elsewhere", "detail": "Lost"},
            True,
            True,
        ),
        ("Tool call failed; its outcome may be unknown", True, True),
        (
            {
                "id": "sequence",
                "compilation_id": "compilation",
                "arms": ["arm"],
                "phase": {
                    "state": "stopped",
                    "failure": {
                        "stage": "admission_unknown",
                        "step": 0,
                        "detail": "Unknown",
                    },
                },
                "requested_steps": [],
                "steps": [],
            },
            False,
            True,
        ),
    ],
)
def test_resume_requires_a_definite_refusal(
    data: object, interrupted: bool, must_stop: bool
) -> None:
    result = RobotToolResult(
        RobotToolCall("motion", "action_sequence", {}),
        "Stopped",
        True,
        interrupted=interrupted,
        data=data,
    )
    assert must_stop_after_tools([result]) is must_stop
