import type { CommandAnswer, LeaseStatus } from "hebbian-rua";
import { test } from "vitest";
import {
  describeLease,
  interpretLeaseAnswer,
  isLeaseHeldElsewhere,
  manualControlBlockedReason,
  manualControlView,
} from "../src/control-logic.js";

test("control logic", async () => {
  function assert(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
  }

  const OWN_LABEL = "viewer 1a2b";
  const ownHold: LeaseStatus = {
    holder: "manual",
    label: OWN_LABEL,
    required_cameras: [],
    on_interruption: "wait_for_approval",
    expires_in_seconds: 12,
  };
  const otherViewer: LeaseStatus = {
    holder: "manual",
    label: "viewer ffff",
    required_cameras: [],
    on_interruption: "wait_for_approval",
    expires_in_seconds: 12,
  };
  const loop: LeaseStatus = {
    holder: "agent",
    label: "stack the cups",
    required_cameras: [],
    on_interruption: "wait_for_approval",
    expires_in_seconds: 9,
  };

  assert(
    manualControlView(null, false, OWN_LABEL).kind === "available",
    "With no holder and no token, manual control is available to turn on",
  );
  assert(
    manualControlView(ownHold, true, OWN_LABEL).kind === "holding",
    "The station naming this viewer, with its token, means it holds control",
  );
  assert(
    manualControlView(null, true, OWN_LABEL).kind === "lapsed",
    "A token with no holder at the station means the lease lapsed",
  );
  const behindARun = manualControlView(loop, false, OWN_LABEL);
  assert(
    behindARun.kind === "elsewhere" &&
      behindARun.heldBy.label === "stack the cups",
    "A run holding control puts this viewer behind it",
  );
  assert(
    manualControlView(otherViewer, false, OWN_LABEL).kind === "elsewhere",
    "Another viewer's manual hold is someone else's, even though the holder is manual",
  );
  assert(
    manualControlView(loop, true, OWN_LABEL).kind === "elsewhere",
    "A token does not help once a run has taken control over",
  );
  assert(
    manualControlView(otherViewer, true, OWN_LABEL).kind === "elsewhere",
    "A token does not help once another viewer took control over",
  );
  assert(
    manualControlView(ownHold, false, OWN_LABEL).kind === "available",
    "Right after releasing, a leftover report naming this viewer is not someone else",
  );

  const holding = manualControlView(ownHold, true, OWN_LABEL);
  assert(
    manualControlBlockedReason(holding) === null,
    "Holding control leaves manual motion unblocked",
  );
  for (const blocked of [
    manualControlView(null, false, OWN_LABEL),
    manualControlView(null, true, OWN_LABEL),
    manualControlView(loop, false, OWN_LABEL),
  ]) {
    assert(
      typeof manualControlBlockedReason(blocked) === "string",
      `Manual motion must be blocked with a reason when ${blocked.kind}`,
    );
  }
  assert(
    manualControlBlockedReason(behindARun)?.includes("stack the cups") === true,
    "The reason names the run that holds control",
  );
  assert(
    manualControlBlockedReason(manualControlView(null, false, OWN_LABEL)) ===
      "Reconnect manual control to move the arm.",
    "An available state tells the user how to start",
  );

  assert(
    describeLease(null, OWN_LABEL) === "No one holds the lease",
    "Idle is said plainly",
  );
  assert(
    describeLease(ownHold, OWN_LABEL) ===
      "Manual control: this viewer holds the lease",
    "This viewer's own hold reads as its own",
  );
  assert(
    describeLease(loop, OWN_LABEL) === 'Agent "stack the cups" holds the lease',
    "An agent is named by its task",
  );
  assert(
    describeLease(otherViewer, OWN_LABEL).includes("viewer ffff"),
    "Another viewer's hold names that viewer",
  );

  const granted = interpretLeaseAnswer({
    kind: "granted",
    lease_token: "secret",
    lease_seconds: 15,
    status: ownHold,
  });
  assert(
    granted.kind === "granted" && granted.leaseSeconds === 15,
    "A grant is a granted claim",
  );
  const refused = interpretLeaseAnswer({ kind: "holder", status: loop });
  assert(
    refused.kind === "refused" && refused.heldBy.holder === "agent",
    "A holder answer carries the current holder",
  );
  assert(
    interpretLeaseAnswer({ kind: "holder", status: null }).kind === "failed",
    "No grant and no holder is a failure",
  );

  function refusal(
    reason: "lease_held_elsewhere" | "not_enabled" | "arm_busy" | "unreachable",
  ): CommandAnswer {
    return { status: "refused", reason, detail: "because" };
  }
  assert(
    isLeaseHeldElsewhere(refusal("lease_held_elsewhere")),
    "A lease_held_elsewhere refusal means the lease is gone",
  );
  for (const reason of ["not_enabled", "arm_busy", "unreachable"] as const)
    assert(
      !isLeaseHeldElsewhere(refusal(reason)),
      `${reason} is about the arm, not about the lease`,
    );
});
