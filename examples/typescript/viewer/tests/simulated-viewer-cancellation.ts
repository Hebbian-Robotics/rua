import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { setTimeout } from "node:timers/promises";
import { apiSchemas } from "hebbian-rua";
import { stationConnection } from "../src/viewer-connection.js";

const [viewerUrl, stationUrl] = process.argv.slice(2);
assert.ok(viewerUrl && stationUrl);

// Supply the browser's URL resolution while using the real HTTP and SSE clients.
const fetchFromNode = globalThis.fetch;
globalThis.fetch = (input, init) =>
  fetchFromNode(
    typeof input === "string" ? new URL(input, viewerUrl) : input,
    init,
  );

const openStreams = new Set<EventSource>();
globalThis.EventSource = class extends EventSource {
  constructor(url: string | URL, init?: EventSourceInit) {
    super(new URL(url, viewerUrl), init);
    openStreams.add(this);
  }
};

async function waitFor<T>(read: () => T | undefined | Promise<T | undefined>) {
  const deadline = AbortSignal.timeout(5_000);
  while (true) {
    const value = await read();
    if (value !== undefined) return value;
    await setTimeout(20, undefined, { signal: deadline });
  }
}

const operationId = randomUUID();
const operationUrl = new URL(`/operations/${operationId}`, stationUrl);
async function operationStatus() {
  const response = await fetchFromNode(operationUrl);
  if (!response.ok) throw new Error(await response.text());
  return apiSchemas.shape.OperationStatus.parse(await response.json());
}

try {
  stationConnection.start();
  const arm = await waitFor(() =>
    stationConnection.isConnected()
      ? stationConnection.getState()?.arms[0]
      : undefined,
  );
  assert.ok(arm.measured_joints);
  const [firstJoint, ...otherJoints] = arm.measured_joints;
  assert.ok(firstJoint !== undefined);
  const lease = await stationConnection.acquireLease("viewer cancellation");
  try {
    const cancellation = new AbortController();
    const admission = await lease.sendCommand(
      arm.arm,
      {
        kind: "move_to_joints",
        joints: [firstJoint + 0.2, ...otherJoints],
        max_joint_speed: 0.05,
      },
      undefined,
      undefined,
      cancellation.signal,
      operationId,
    );
    assert.equal(admission.status, "accepted");
    assert.equal((await operationStatus()).state, "active");
    cancellation.abort();

    const stopped = await waitFor(async () => {
      const operation = await operationStatus();
      return operation.state === "finished" ? operation : undefined;
    });
    const phase = stopped.records[0]?.record.phase;
    assert.ok(phase?.kind === "finished");
    assert.equal(phase.completion.outcome, "stopped");
  } finally {
    await lease.return();
  }
} finally {
  for (const stream of openStreams) stream.close();
}
