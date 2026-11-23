import json

from hebbian_rua import (
    RobotOutput,
    default_formatter,
    full_formatter,
    render_output,
    select_output_cameras,
    summarize_output,
)
from hebbian_rua.client.contract.station_api import (
    Achieved,
    Checkpoint,
    CommandCompletion,
    CommandRecord,
    FinishedCommand,
    KnownCommandOutcome,
    RecordedMove,
)
from hebbian_rua.client.llm.response_format import result_from_data
from hebbian_rua.client.llm.types import RobotToolCall


def test_motion_summary_keeps_failure_and_achieved_pose_without_losing_raw_history() -> (
    None
):
    record = CommandRecord(
        command_id="move-1",
        label="move_to_joints",
        accepted_unix_nano="123123456789",
        checkpoints=[Checkpoint(label="start", measured_joints=[0.123456789])],
        command=RecordedMove(destination_joints=[0.3], joint_speed=0.5),
        phase=FinishedCommand(
            completion=CommandCompletion(
                finished_unix_nano="124123456789",
                outcome=KnownCommandOutcome.stopped_short,
                detail="Object contact",
            ),
            achieved=Achieved(joints=[0.123456789], sampled_at=124.123456789),
        ),
    )
    result = result_from_data(
        RobotToolCall("call-1", "set_arm_gripper", {}),
        record.model_dump(mode="json"),
        is_error=True,
    )
    summary = json.loads(result.text)
    assert summary["completion"] == {
        "outcome": "stopped_short",
        "detail": "Object contact",
    }
    assert summary["achieved"]["joints_degrees"] == [7.1]
    assert "checkpoints" not in summary
    assert "accepted_unix_nano" not in summary
    assert "command_id" not in summary
    assert result.is_error
    assert result.data == record.model_dump(mode="json")
    detailed = full_formatter(RobotOutput("result", result.data, call=result.call))
    assert json.loads(detailed.text)["command_id"] == "move-1"
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
        cameras={"table": b"jpeg", "side": b"side-jpeg"},
        notes=("Recovered connection",),
        missing_optional_cameras=("wrist",),
        camera_intervals={
            "table": (123.123456789, 123.234567891),
            "side": (124.123456789, 124.234567891),
        },
    )
    transformed = select_output_cameras(summarize_output(output), {"table"})
    assert isinstance(transformed.data, dict)
    assert transformed.data["arms"][0]["measured_joints"] == [0.123456789]
    summary = render_output(transformed)
    assert summary.cameras == {"table": b"jpeg"}
    assert "required_camera_unavailable" in summary.text
    assert "measured_joint_velocities" not in summary.text
    assert "measured_joint_efforts" not in summary.text
    assert "test fault" in summary.text
    assert "Recovered connection" in summary.text
    assert "Optional cameras unavailable: wrist" in summary.text
    assert "123.123" in summary.text
    assert "124.123" not in summary.text
    assert '"measured_joints_degrees":[7.1]' in summary.text
    assert "model_points" not in summary.text
    assert "last_command" not in summary.text
    assert summary.text.count('"arm":"left"') == 1
    assert "123.5" in summary.text
    detailed = full_formatter(output)
    assert "0.123456789" in detailed.text
    assert '"measured_joint_velocities": [0.5]' in detailed.text
    assert '"measured_joint_efforts": [0.42]' in detailed.text
    assert "model_points" in detailed.text
    assert detailed.cameras == {"table": b"jpeg", "side": b"side-jpeg"}
    assert "124.123456789" in detailed.text
    assert output.camera_intervals["side"] == (124.123456789, 124.234567891)
    assert state["arms"][0]["measured_joints"] == [0.123456789]


def test_sequence_summary_reports_progress_and_each_step_outcome() -> None:
    payload = {
        "id": "sequence-1",
        "arms": ["left"],
        "phase": {
            "state": "stopped",
            "failure": {"stage": "execution", "step": 19, "detail": "Motion limited"},
        },
        "requested_steps": [{"gripper_opening": 1.0}] * 30,
        "steps": [[{"completion": {"outcome": "completed"}}]] * 19
        + [
            [
                {
                    "completion": {
                        "outcome": "stopped_short",
                        "detail": "Object contact",
                    },
                    "goal_status": "partial",
                }
            ]
        ],
    }
    summary = json.loads(default_formatter(RobotOutput("result", payload)).text)
    assert summary["requested_step_count"] == 30
    assert summary["recorded_step_count"] == 20
    assert summary["phase"]["failure"]["detail"] == "Motion limited"
    assert len(summary["steps"]) == 20
    assert summary["steps"][0][0]["completion"]["outcome"] == "completed"
    assert summary["steps"][-1][0]["completion"]["outcome"] == "stopped_short"
    assert "requested_steps" not in summary
    assert "id" not in summary
    assert payload["id"] == "sequence-1"
    assert len(payload["steps"]) == 20
