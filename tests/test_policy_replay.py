import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest

from examples.mujoco_sim.so101_model import So101MujocoModelPin
from tests.fixture_station import resolve_simulator_arm

REPOSITORY = Path(__file__).resolve().parent.parent
ASSETS = REPOSITORY / ".cache" / "so101-policy-assets"
# WHY: The golden trace comes from so-supermimic's reference controller and is
# distributed separately from this repository, alongside the checkpoint assets.
GOLDEN_DIRECTORY = Path(
    os.environ.get(
        "SO101_POLICY_GOLDEN_DIR",
        "~/golden-data/so101-soft-gripper-camera-servo-fit-30000",
    )
).expanduser()
CHECKPOINT = ASSETS / "so101-servo-fit-30000-updates.pt"
SOURCE_MANIFEST = ASSETS / "source-manifest.json"
ACTION_TOLERANCE = 2e-4
JOINT_TARGET_TOLERANCE = 1e-5


def test_replay_matches_the_reference_controller(
    server_binary: Path, tmp_path: Path
) -> None:
    pytest.importorskip("onnx")
    pytest.importorskip("torch")
    golden_path = GOLDEN_DIRECTORY / "golden.npz"
    training_model = So101MujocoModelPin.load().model_path
    if (
        not golden_path.is_file()
        or not CHECKPOINT.is_file()
        or not training_model.is_file()
    ):
        pytest.skip("Golden data or the policy checkpoint assets are unavailable")
    if not os.environ.get("ORT_DYLIB_PATH"):
        pytest.skip("Set ORT_DYLIB_PATH to run the optional policy replay")
    golden = np.load(golden_path)
    recorded = json.loads((GOLDEN_DIRECTORY / "golden.json").read_text())

    onnx_path = tmp_path / "policy.onnx"
    subprocess.run(
        [
            sys.executable,
            str(REPOSITORY / "scripts" / "export_so101_policy_onnx.py"),
            "--checkpoint",
            str(CHECKPOINT),
            "--source-manifest",
            str(SOURCE_MANIFEST),
            "--training-model",
            str(training_model),
            "--output",
            str(onnx_path),
        ],
        check=True,
        capture_output=True,
    )
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
                "checkpoint_sha256": recorded["checkpoint_sha256"],
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
    # CONSTRAINT: The reference also clips to the training model's joint range; the executor's
    # own limits apply to the command later, so compare where it did not clip.
    station_unclipped = golden["measured_joints"] + recorded["action_step"] * np.clip(
        golden["action_unclamped"], -1, 1
    )
    # CONSTRAINT: The reference computes its targets in single precision.
    not_clipped = np.abs(golden["joint_target"] - station_unclipped) < 1e-6
    assert not_clipped.mean() > 0.9
    target_error = np.abs(targets - golden["joint_target"])[not_clipped].max()
    assert target_error < JOINT_TARGET_TOLERANCE, target_error
