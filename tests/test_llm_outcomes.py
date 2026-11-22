import json
from pathlib import Path

from hebbian_rua.client.contract.station_api import (
    CommandOutcomeUnknown,
    CommandRecord,
    RecordedMove,
)
from hebbian_rua.client.llm.robot_tools import records_result
from hebbian_rua.client.llm.types import RobotToolCall


def test_group_outcomes_match_the_shared_examples() -> None:
    examples = json.loads(
        (Path(__file__).parent / "fixtures/model-command-outcomes.json").read_text()
    )
    for example in examples:
        result = records_result(
            RobotToolCall(example["name"], "move_arms", {}),
            [CommandRecord.model_validate(example["record"])],
            grouped=True,
        )
        assert result.is_error is example["expected_error"]
        assert json.loads(result.text)["status"] == example["expected_status"]
        single = records_result(
            RobotToolCall(example["name"], "set_arm_gripper", {}),
            [CommandRecord.model_validate(example["record"])],
        )
        assert single.is_error is example["expected_error"]
        assert (
            json.loads(single.text)["completion"]["outcome"]
            == example["record"]["phase"]["completion"]["outcome"]
        )


def test_an_interrupted_move_requires_recovery_not_a_hard_fault_stop() -> None:
    result = records_result(
        RobotToolCall("1", "move_to_joints", {}),
        [
            CommandRecord(
                command_id="c",
                label="move_to_joints",
                accepted_at=0.0,
                checkpoints=[],
                command=RecordedMove(target_joints=[0.1], joint_speed=1.0),
                phase=CommandOutcomeUnknown(finished_at=1.0),
            )
        ],
    )
    assert result.is_error
    assert result.interrupted
    assert not result.halt
    assert '"outcome":"interrupted"' in result.text
