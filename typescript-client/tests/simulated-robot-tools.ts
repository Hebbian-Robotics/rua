import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRobotTools, createStationConnection } from "hebbian-rua";

const examples = JSON.parse(
  readFileSync(
    new URL("../../tests/fixtures/model-tool-simulation.json", import.meta.url),
    "utf8",
  ),
) as {
  limited_position_millimeters: number[];
  partial_gripper_opening: number;
  requested_speed_degrees_per_second: number;
};
const server = process.argv[2];
assert.ok(server);
const connection = createStationConnection(server, "agent");
await connection.waitUntilConnected();
const lease = await connection.acquireLease("SDK tool simulation");
const robot = createRobotTools({
  tools: [
    "action_sequence",
    "preview_pose",
    "move_arms",
    "set_arm_gripper",
    "rehome",
    "stop",
    "observe",
  ],
  connection,
  lease,
  arm: "arm",
});
try {
  const context = await robot.context();
  assert.ok(context.jointNames.length > 0);
  assert.ok(context.description);
  const state = await connection.state();
  const position = state.arms.find((arm) => arm.arm === "arm")?.tool?.position;
  assert.ok(position);
  const httpPreviews = await connection.previewPoses({
    arm: "arm",
    poses: [{ id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", pose: { position } }],
  });
  assert.equal(
    httpPreviews.results[0]?.id,
    "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  );
  // Reorder real station answers to exercise the adapter's ID-based correlation.
  const stationPreviewPoses = connection.previewPoses.bind(connection);
  connection.previewPoses = async (request) => {
    assert.equal(
      new Set(request.poses.map((candidate) => candidate.id)).size,
      request.poses.length,
    );
    const response = await stationPreviewPoses(request);
    return { ...response, results: response.results.toReversed() };
  };
  const [preview] = await robot.execute([
    {
      id: "preview",
      name: "preview_pose",
      arguments: {
        poses: [
          {
            pose: {
              position_mm: position.map((coordinate) => coordinate * 1000),
            },
          },
          {
            pose: { position_mm: [50000, 50000, 50000] },
          },
        ],
      },
    },
  ]);
  assert.ok(preview);
  assert.equal(preview.isError, false);
  const previewResults = (preview.data as { results: { answer: unknown }[] })
    .results;
  assert.equal(previewResults.length, 2);
  assert.ok(
    previewResults.every(
      (candidate) => Object.keys(candidate).join() === "answer",
    ),
  );
  const orderedPreview = preview.modelData as {
    results: {
      answer: {
        projection: {
          outcome: {
            kind: string;
            within_tolerance: boolean;
            pose: { position_mm: number[] };
          };
        };
      };
    }[];
  };
  assert.equal(
    orderedPreview.results[1]?.answer.projection.outcome.kind,
    "projected",
  );
  assert.equal(
    orderedPreview.results[0]?.answer.projection.outcome.within_tolerance,
    true,
  );
  assert.equal(
    orderedPreview.results[1]?.answer.projection.outcome.within_tolerance,
    false,
  );
  const projectedPosition =
    orderedPreview.results[0]?.answer.projection.outcome.pose.position_mm;
  assert.ok(projectedPosition);
  position.forEach((coordinate, index) => {
    assert.ok(
      Math.abs((projectedPosition[index] ?? Infinity) - coordinate * 1000) < 1,
    );
  });
  connection.previewPoses = stationPreviewPoses;
  const [gripper] = await robot.execute([
    {
      id: "gripper",
      name: "set_arm_gripper",
      arguments: {
        arm: "arm",
        opening: examples.partial_gripper_opening,
        max_joint_speed_degrees_per_second:
          examples.requested_speed_degrees_per_second,
      },
    },
  ]);
  assert.equal(gripper?.isError, false);
  assert.ok(gripper?.data && typeof gripper.data === "object");
  assert.equal(
    (gripper.modelData as { joint_speed_degrees_per_second: number })
      .joint_speed_degrees_per_second,
    Math.round(context.maxJointSpeedDegreesPerSecond * 10) / 10,
  );
  const homeJoints = (await connection.state()).arms.find(
    (arm) => arm.arm === "arm",
  )?.rest_pose;
  assert.ok(homeJoints);
  const displacedJoints = [...homeJoints];
  displacedJoints[0] = (displacedJoints[0] ?? 0) + 0.15;
  const [movement] = await robot.execute([
    {
      id: "move",
      name: "action_sequence",
      arguments: {
        steps: [
          {
            movement: {
              joints_degrees: displacedJoints.map(
                (joint) => (joint * 180) / Math.PI,
              ),
            },
          },
        ],
      },
    },
  ]);
  assert.equal(movement?.isError, false);
  const beforeRehome = (await connection.state()).arms.find(
    (arm) => arm.arm === "arm",
  );
  assert.ok(beforeRehome);
  const openingBeforeRehome = beforeRehome.gripper_opening;
  assert.ok(openingBeforeRehome !== null && openingBeforeRehome > 0.02);
  displacedJoints.forEach((joint, index) => {
    assert.ok(
      Math.abs((beforeRehome.measured_joints[index] ?? Infinity) - joint) <
        0.02,
    );
  });
  const rehomed = await lease.arm("arm").rehome();
  assert.equal(rehomed.status, "accepted");
  const homeRecord = rehomed.records[0]?.record;
  assert.equal(homeRecord?.label, "rehome");
  assert.ok(homeRecord?.phase.kind === "finished");
  assert.equal(homeRecord.phase.completion.outcome, "completed");
  assert.ok(
    homeRecord.phase.achieved?.joints.every(
      (joint, index) => Math.abs(joint - (homeJoints[index] ?? 0)) < 0.02,
    ),
  );
  assert.ok(
    Math.abs(
      ((await connection.state()).arms.find((arm) => arm.arm === "arm")
        ?.gripper_opening ?? 0) - openingBeforeRehome,
    ) < 0.02,
  );
  const [refused] = await robot.execute([
    {
      id: "unknown-arm",
      name: "set_arm_gripper",
      arguments: { arm: "unknown", opening: 0.5 },
    },
  ]);
  assert.equal(refused?.isError, true);
  assert.ok(refused?.text.toLowerCase().includes("unknown"));
  const joints = (await connection.state()).arms.find(
    (arm) => arm.arm === "arm",
  )?.measured_joints;
  assert.ok(joints);
  const [group] = await robot.execute([
    {
      id: "group",
      name: "move_arms",
      arguments: {
        targets: {
          arm: {
            joints_degrees: joints.map((joint) => (joint * 180) / Math.PI),
          },
        },
      },
    },
  ]);
  assert.equal(group?.isError, false);
  const measuredJoints = (await connection.state()).arms.find(
    (arm) => arm.arm === "arm",
  )?.measured_joints;
  assert.ok(measuredJoints);
  joints.forEach((joint, index) => {
    assert.ok(Math.abs((measuredJoints[index] ?? Infinity) - joint) < 0.02);
  });
  const currentPose = (await connection.state()).arms.find(
    (item) => item.arm === "arm",
  )?.tool;
  assert.ok(currentPose);
  const [limited] = await robot.execute([
    {
      id: "limited",
      name: "move_arms",
      arguments: {
        targets: {
          arm: {
            position_mm: examples.limited_position_millimeters,
            orientation: { quaternion: currentPose.quaternion },
          },
        },
      },
    },
  ]);
  assert.ok(limited);
  assert.equal(limited.isError, true);
  assert.equal((limited.data as { status: string }).status, "limited");
} finally {
  robot.close();
  connection.close();
}
