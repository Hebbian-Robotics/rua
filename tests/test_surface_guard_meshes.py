# SAFETY: If the training and station meshes drift apart, the table guard could
# reject safe reaches or allow reaches that the policy assumes are unsafe. Compare
# both models' lowest moving points at the same joints, excluding the base.

import json
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

import numpy as np
import pytest

from examples.mujoco_sim.so101_model import So101MujocoModelPin
from tests.fixture_station import resolve_simulator_arm

if TYPE_CHECKING:
    import mujoco

ARM_JOINTS = ("shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll")
# CONSTRAINT: The joint ranges the policy's training goals are drawn from.
GOAL_JOINT_LOWER = np.array([-1.1, -0.6, -0.7, -0.65, -1.0])
GOAL_JOINT_UPPER = np.array([1.1, 0.7, 0.9, 0.65, 1.0])
CONFIGURATIONS = 300
# WHY: Millimetres of disagreement the guard's 10 mm leeway comfortably absorbs. A few
# configurations differ more (the gripper opening is not varied here), so the
# typical difference and the 95th percentile are held, not the worst case.
MEDIAN_TOLERANCE_MILLIMETRES = 2.0
P95_TOLERANCE_MILLIMETRES = 5.0


def lowest_moving_mesh_point(model: "mujoco.MjModel", data: "mujoco.MjData") -> float:
    import mujoco

    lowest = np.inf
    for geom in range(model.ngeom):
        if model.geom_type[geom] != mujoco.mjtGeom.mjGEOM_MESH:
            continue
        if model.body(model.geom_bodyid[geom]).name in ("world", "base_link"):
            continue
        mesh = model.geom_dataid[geom]
        start = model.mesh_vertadr[mesh]
        vertices = model.mesh_vert[start : start + model.mesh_vertnum[mesh]]
        world = (data.geom_xmat[geom].reshape(3, 3) @ vertices.T).T + data.geom_xpos[
            geom
        ]
        lowest = min(lowest, float(world[:, 2].min()))
    return lowest


def station_lowest_points(
    binary: Path, urdf: Path, samples: np.ndarray, directory: Path
) -> list[float]:
    probe = directory / "probe.json"
    probe.write_text(
        json.dumps(
            {
                "urdf": str(urdf),
                "driven_joints": list(ARM_JOINTS),
                "samples": samples.tolist(),
            }
        )
    )
    result = subprocess.run(
        [str(binary), "probe-robot", "--trace", str(probe)],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def test_station_and_training_meshes_agree_where_the_arm_is_above_the_table(
    server_binary: Path, tmp_path: Path
) -> None:
    mujoco = pytest.importorskip("mujoco")
    config = resolve_simulator_arm(server_binary)
    training_model = So101MujocoModelPin.load().model_path
    if not training_model.is_file():
        pytest.skip("SO-101 MuJoCo model not checked out; run the MuJoCo fixture once")
    training = mujoco.MjModel.from_xml_path(str(training_model))
    training_data = mujoco.MjData(training)
    addresses = [int(training.joint(name).qposadr[0]) for name in ARM_JOINTS]
    random_generator = np.random.default_rng(0)
    samples = random_generator.uniform(
        GOAL_JOINT_LOWER, GOAL_JOINT_UPPER, (CONFIGURATIONS, len(ARM_JOINTS))
    )
    differences_millimetres = []
    for joints, station_lowest in zip(
        samples,
        station_lowest_points(server_binary, config.urdf, samples, tmp_path),
        strict=True,
    ):
        training_data.qpos[addresses] = joints
        mujoco.mj_forward(training, training_data)
        differences_millimetres.append(
            (station_lowest - lowest_moving_mesh_point(training, training_data)) * 1000
        )
    absolute = np.abs(differences_millimetres)
    assert np.median(absolute) <= MEDIAN_TOLERANCE_MILLIMETRES
    assert np.percentile(absolute, 95) <= P95_TOLERANCE_MILLIMETRES
