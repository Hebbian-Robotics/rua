import json
import os
import subprocess
from pathlib import Path

import numpy as np
import pytest

from examples.mujoco_sim.so101_model import So101MujocoModelPin
from tests.fixture_station import resolve_simulator_arm

REPOSITORY = Path(__file__).resolve().parent.parent
ACTION_TOLERANCE = 2e-4
JOINT_TARGET_TOLERANCE = 1e-5


def test_replay_matches_the_reference_controller(
    server_binary: Path, tmp_path: Path
) -> None:
    golden_directory_setting = os.environ.get("SO101_POLICY_GOLDEN_DIR")
    if not golden_directory_setting:
        pytest.skip(
            "Set SO101_POLICY_GOLDEN_DIR to the reference replay data directory"
        )
    golden_directory = Path(golden_directory_setting).expanduser()
    golden_path = golden_directory / "golden.npz"
    # CONSTRAINT: Replay needs the ONNX model that produced the reference trace.
    onnx_path = golden_directory / "policy.onnx"
    training_model = So101MujocoModelPin.load().model_path
    if (
        not golden_path.is_file()
        or not onnx_path.is_file()
        or not training_model.is_file()
    ):
        pytest.skip("Golden data or its policy model are unavailable")
    if not os.environ.get("ORT_DYLIB_PATH"):
        pytest.skip("Set ORT_DYLIB_PATH to run the optional policy replay")
    golden = np.load(golden_path)
    recorded = json.loads((golden_directory / "golden.json").read_text())

    policy_profile = REPOSITORY / "examples/mujoco_sim/profiles/policy.toml"
    replay_profile = tmp_path / "policy.toml"
    replay_profile.write_text(
        f"extends = {json.dumps(str(policy_profile))}\n"
        f"[policy]\nonnx = {json.dumps(str(onnx_path))}\n"
    )
    config = resolve_simulator_arm(server_binary, replay_profile)
    trace_path = tmp_path / "replay.json"
    trace_path.write_text(
        json.dumps(
            {
                "urdf": str(config.urdf),
                "arm_joints": config.driven_joints[: len(config.station.joint_names)],
                "tool_link": config.tool_link,
                "onnx": str(onnx_path),
                "steps": [
                    {
                        "reset": bool(golden["reset"][index]),
                        "positions": golden["measured_joints"][index].tolist(),
                        "velocities": golden["measured_velocities"][index].tolist(),
                        "target_position": golden["target_position"][index].tolist(),
                        "target_quaternion": golden["target_quaternion"][
                            index
                        ].tolist(),
                    }
                    for index in range(len(golden["step"]))
                ],
            }
        )
    )
    result = subprocess.run(
        [str(server_binary), "replay-policy", "--trace", str(trace_path)],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    steps = json.loads(result.stdout)
    actions = np.array([step["action"] for step in steps])
    targets = np.array([step["joint_target"] for step in steps])

    action_error = np.abs(actions - golden["action_unclamped"]).max()
    assert action_error < ACTION_TOLERANCE, action_error
    # CONSTRAINT: The reference also clips to the training model's joint range; the controller's
    # own limits apply to the command later, so compare where it did not clip.
    station_unclipped = golden["measured_joints"] + recorded["action_step"] * np.clip(
        golden["action_unclamped"], -1, 1
    )
    # CONSTRAINT: The reference computes its targets in single precision.
    not_clipped = np.abs(golden["joint_target"] - station_unclipped) < 1e-6
    assert not_clipped.mean() > 0.9
    target_error = np.abs(targets - golden["joint_target"])[not_clipped].max()
    assert target_error < JOINT_TARGET_TOLERANCE, target_error
