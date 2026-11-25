import assert from "node:assert/strict";
import {
  apiSchemas,
  commandResult,
  createStationConnection,
} from "hebbian-rua";

const [stationUrl, arm, operation] = process.argv.slice(2);
assert.ok(stationUrl && arm);
assert.ok(
  operation === "command" ||
    operation === "action_sequence" ||
    operation === "compiled_sequence",
);

let selectedStation = stationUrl;
const connection = createStationConnection(() => selectedStation, "agent");
try {
  await connection.waitUntilConnected();
  const lease = await connection.acquireLease("lost reply test");
  try {
    if (operation !== "command") {
      const gripper = { gripper_opening: 1 };
      const source = { steps: [{ arms: { [arm]: gripper } }] };
      const currentArm = (await connection.state()).arms.find(
        (item) => item.arm === arm,
      );
      assert.ok(currentArm);
      const revisions = { [arm]: currentArm.revision };
      let compilationId: string | undefined;
      const submissionPath =
        operation === "compiled_sequence"
          ? "/compilations/run"
          : "/action_sequence";
      if (operation === "compiled_sequence") {
        const response = await fetch(
          new URL("/compilations/compile", stationUrl),
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(source),
          },
        );
        assert.ok(response.ok);
        const compilation = apiSchemas.shape.CompilationOutcome.parse(
          await response.json(),
        );
        assert.ok("artifact" in compilation);
        compilationId = compilation.artifact.compilation_id;
      }
      const fetchFromNetwork = globalThis.fetch;
      globalThis.fetch = async (input, init) => {
        try {
          return await fetchFromNetwork(input, init);
        } catch (error) {
          if (new URL(String(input)).pathname === submissionPath) {
            selectedStation = "http://127.0.0.1:1";
            gripper.gripper_opening = 0;
            revisions[arm] = currentArm.revision + 1;
          }
          throw error;
        }
      };
      const sequence = await (async () => {
        try {
          const execution = compilationId
            ? await lease.startCompiledExecution(compilationId, revisions)
            : await lease.startSequenceExecution(source);
          return await execution.wait();
        } finally {
          globalThis.fetch = fetchFromNetwork;
          selectedStation = stationUrl;
        }
      })();
      assert.equal(sequence.phase.state, "completed");
      assert.equal(sequence.steps.length, 1);
      await connection.waitForState((state) =>
        state.arms.some(
          (item) =>
            item.arm === arm &&
            Math.abs((item.gripper_opening ?? -1) - 1) < 0.02,
        ),
      );
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
