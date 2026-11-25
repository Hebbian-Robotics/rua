import assert from "node:assert/strict";
import { commandResult, createStationConnection } from "hebbian-rua";

const [stationUrl, arm, operation] = process.argv.slice(2);
assert.ok(stationUrl && arm);
assert.ok(operation === "command" || operation === "action_sequence");

const connection = createStationConnection(stationUrl, "agent");
try {
  await connection.waitUntilConnected();
  const lease = await connection.acquireLease("lost reply test");
  try {
    if (operation === "action_sequence") {
      const id = await lease.startActionSequence({
        steps: [{ arms: { [arm]: { gripper_opening: 1 } } }],
      });
      const sequence = await connection.waitForActionSequence(id);
      assert.equal(sequence.phase.state, "completed");
      assert.equal(sequence.steps.length, 1);
      process.stdout.write(sequence.id);
    } else {
      const answer = await lease.arm(arm).setGripper(1);
      assert.equal(answer.status, "accepted");
      const record = answer.records[0]?.record;
      assert.ok(record);
      assert.equal(commandResult(record)?.outcome, "completed");
      const state = (await connection.state()).arms.find(
        (item) => item.arm === arm,
      );
      assert.equal(state?.last_command?.command_id, record.command_id);
      assert.ok(Math.abs((state.gripper_opening ?? -1) - 1) < 0.02);
      process.stdout.write(record.command_id);
    }
  } finally {
    await lease.return();
  }
} finally {
  connection.close();
}
