"""Model views stay concise while application data remains complete and exact."""

import json

from hebbian_rua import RobotOutput, default_formatter, full_formatter
from hebbian_rua.client.contract.station_api import (
    Achieved,
    Checkpoint,
    CommandCompletion,
    CommandOutcome,
    CommandRecord,
    TargetStatus,
)
from hebbian_rua.client.llm.robot_tools import records_result
from hebbian_rua.client.llm.types import RobotToolCall


def test_motion_summary_keeps_failure_and_achieved_pose_without_losing_raw_history() -> (
    None
):
    record = CommandRecord(
        command_id="move-1",
        label="move_to_joints",
        accepted_at=123.123456789,
        checkpoints=[Checkpoint(label="start", measured_joints=[0.123456789])],
        completion=CommandCompletion(
            finished_at=124.123456789,
            outcome=CommandOutcome.stopped_short,
            detail="Object contact",
        ),
        achieved=Achieved(joints=[0.123456789], sampled_at=124.123456789),
        target_status=TargetStatus.partial,
    )
    result = records_result(RobotToolCall("call-1", "set_arm_gripper", {}), [record])
    summary = json.loads(result.text)
    assert summary["completion"] == {
        "outcome": "stopped_short",
        "detail": "Object contact",
    }
    assert summary["achieved"]["joints_degrees"] == [7.1]
    assert "checkpoints" not in summary
    assert "accepted_at" not in summary
    assert result.is_error
    assert result.data == record.model_dump(mode="json")
    detailed = full_formatter(RobotOutput("result", result.data, call=result.call))
    assert json.loads(detailed.text)["checkpoints"][0]["measured_joints"] == [
        0.123456789
    ]


def test_observation_summary_preserves_interlocks_cameras_and_recovery_context() -> (
    None
):
    state = {
        "robot": "test",
        "joint_names": ["shoulder"],
        "max_joint_speed": 0.5,
        "arms": [
            {
                "arm": "left",
                "mode": "holding",
                "measured_joints": [0.123456789],
                "measured_joint_velocities": [0.5],
                "measured_joint_efforts": [0.42],
                "interlocks": [
                    {"kind": "required_camera_unavailable", "camera": "table"}
                ],
                "fault": "test fault",
                "revision": 4,
                "tool": {
                    "position": [0.123456789, 0, 0],
                    "quaternion": [0, 0, 0, 1],
                    "model_points": [1, 2, 3],
                },
                "last_command": {"command_id": "old"},
            }
        ],
    }
    output = RobotOutput(
        "observation",
        state,
        selected_arm="left",
        cameras={"table": b"jpeg"},
        notes=("Recovered connection",),
        missing_optional_cameras=("wrist",),
        camera_intervals={"table": (123.123456789, 123.234567891)},
    )
    summary = default_formatter(output)
    assert summary.cameras == {"table": b"jpeg"}
    assert "required_camera_unavailable" in summary.text
    assert "measured_joint_velocities" not in summary.text
    assert "measured_joint_efforts" not in summary.text
    assert "test fault" in summary.text
    assert "Recovered connection" in summary.text
    assert "Optional cameras unavailable: wrist" in summary.text
    assert "123.123" in summary.text
    assert "model_points" not in summary.text
    assert "last_command" not in summary.text
    assert summary.text.count('"arm":"left"') == 1
    assert "123.5" in summary.text
    detailed = full_formatter(output)
    assert "0.123456789" in detailed.text
    assert '"measured_joint_velocities": [0.5]' in detailed.text
    assert '"measured_joint_efforts": [0.42]' in detailed.text
    assert "model_points" in detailed.text
    assert detailed.cameras == summary.cameras
    assert state["arms"][0]["measured_joints"] == [0.123456789]


def test_sequence_summary_reports_progress_and_last_failure_without_repeating_steps() -> (
    None
):
    payload = {
        "id": "sequence-1",
        "arm": "left",
        "phase": {"state": "stopped", "detail": "Motion limited"},
        "requested_steps": [{"gripper_opening": 1.0}] * 30,
        "steps": [[{"completion": {"outcome": "completed"}}]] * 19
        + [
            [
                {
                    "completion": {
                        "outcome": "stopped_short",
                        "detail": "Object contact",
                    },
                    "target_status": "partial",
                }
            ]
        ],
    }
    summary = json.loads(default_formatter(RobotOutput("result", payload)).text)
    assert summary["requested_step_count"] == 30
    assert summary["recorded_step_count"] == 20
    assert summary["phase"]["detail"] == "Motion limited"
    assert summary["last_step"][0]["completion"]["outcome"] == "stopped_short"
    assert "steps" not in summary
    assert "requested_steps" not in summary
    assert len(payload["steps"]) == 20
