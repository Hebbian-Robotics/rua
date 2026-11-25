import {
  type CommandAnswer,
  type Lease,
  type LeaseAnswer,
  LeaseHeldElsewhereError,
  type LeaseStatus,
} from "hebbian-rua";
import { test } from "vitest";
import { createManualLease } from "../src/manual-lease.js";

const holder: LeaseStatus = {
  holder: "manual",
  label: "viewer 0000",
  required_cameras: [],
  on_interruption: "wait_for_approval",
  expires_in_seconds: 15,
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function fakeConnection() {
  const claims: string[] = [];
  let returns = 0;
  const endedListeners: Array<() => void> = [];
  const sent: string[] = [];
  function answered(): Lease {
    if (state.answer instanceof Error) throw state.answer;
    if (state.answer.kind !== "granted")
      throw new LeaseHeldElsewhereError(state.answer);
    return {
      leaseSeconds: state.answer.lease_seconds,
      status: state.answer.status,
      return: async () => {
        returns += 1;
      },
      sendCommand: async (arm: string) => {
        sent.push(arm);
        return state.commandAnswer;
      },
    } as unknown as Lease;
  }
  const state: { answer: LeaseAnswer | Error; commandAnswer: CommandAnswer } = {
    answer: {
      kind: "granted",
      lease_token: "t",
      lease_seconds: 15,
      status: holder,
    },
    commandAnswer: { status: "accepted", records: [] },
  };
  return {
    claims,
    sent,
    state,
    returns: () => returns,
    loseLease: () => {
      for (const listener of endedListeners) listener();
    },
    connection: {
      acquireLease: async (label: string) => {
        claims.push(label);
        return answered();
      },
      onLeaseLost: (listener: () => void) => {
        endedListeners.push(listener);
        return () => {
          const index = endedListeners.indexOf(listener);
          if (index >= 0) endedListeners.splice(index, 1);
        };
      },
    },
  };
}

test("a granted lease is held, announced once and returned on request", async () => {
  const fake = fakeConnection();
  const lease = createManualLease(fake.connection, () => "0000");
  let changes = 0;
  lease.onChange(() => {
    changes += 1;
  });
  assert(lease.label === "viewer 0000", "The label is per page load");
  assert((await lease.acquire()).kind === "granted", "The lease is granted");
  assert(lease.holdsLease() && changes === 1, "Held, and announced once");
  assert(
    fake.claims[0] === lease.label,
    "The station receives the viewer label",
  );

  await lease.returnLease();
  assert(!lease.holdsLease() && fake.returns() === 1, "Returned");
  await lease.returnLease();
  assert(fake.returns() === 1, "Returning without a lease sends nothing");
});

test("a refused or unreachable acquire holds nothing", async () => {
  const fake = fakeConnection();
  const lease = createManualLease(fake.connection);
  fake.state.answer = {
    kind: "holder",
    status: {
      holder: "agent",
      label: "stack the cups",
      required_cameras: [],
      on_interruption: "wait_for_approval",
      expires_in_seconds: 9,
    },
  };
  const refused = await lease.acquire();
  assert(
    refused.kind === "refused" && refused.heldBy.label === "stack the cups",
    "A refused acquire reports the holder",
  );
  fake.state.answer = { kind: "holder", status: null };
  assert(
    (await lease.acquire()).kind === "failed",
    "A vanished holder cannot grant control",
  );
  fake.state.answer = new Error("network down");
  const failed = await lease.acquire();
  assert(
    failed.kind === "failed" && failed.message.includes("network down"),
    "An unreachable station is a failure with the reason",
  );
  assert(!lease.holdsLease(), "Nothing was granted");
});

test("the lease ends when the station says so or a command is refused for it", async () => {
  const fake = fakeConnection();
  const lease = createManualLease(fake.connection);
  await lease.acquire();
  fake.loseLease();
  assert(!lease.holdsLease(), "lease_lost or a dropped connection ends it");

  await lease.acquire();
  fake.state.commandAnswer = {
    status: "refused",
    reason: "not_enabled",
    detail: "Arm is disabled",
  };
  await lease.send("follower", { kind: "set_gripper", opening: 1 });
  assert(lease.holdsLease(), "A refusal about the arm keeps the lease");
  fake.state.commandAnswer = {
    status: "refused",
    reason: "lease_held_elsewhere",
    detail: "An agent holds the lease",
  };
  await lease.send("follower", { kind: "set_gripper", opening: 1 });
  assert(!lease.holdsLease(), "lease_held_elsewhere ends it");
});

test("a command that moves an arm goes out under the held lease only", async () => {
  const fake = fakeConnection();
  const lease = createManualLease(fake.connection);
  let rejected = false;
  await lease
    .send("follower", { kind: "set_gripper", opening: 1 })
    .catch(() => {
      rejected = true;
    });
  assert(
    rejected && fake.sent.length === 0,
    "Nothing is sent without the lease",
  );
  await lease.acquire();
  await lease.send("follower", { kind: "set_gripper", opening: 1 });
  assert(fake.sent[0] === "follower", "The held lease sends it");
});
