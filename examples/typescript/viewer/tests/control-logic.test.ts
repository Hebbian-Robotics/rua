import type { CommandAnswer, LeaseStatus } from "hebbian-rua";
import { expect, test } from "vitest";
import {
  isLeaseHeldElsewhere,
  manualControlBlockedReason,
  manualControlView,
} from "../src/control-logic.js";

const ownLabel = "viewer 1a2b";
const ownLease: LeaseStatus = {
  holder: "manual",
  label: ownLabel,
  required_cameras: [],
  on_interruption: "wait_for_approval",
  expires_in_seconds: 12,
};
const otherViewer = { ...ownLease, label: "viewer ffff" };
const agentLease = {
  ...ownLease,
  holder: "agent",
  label: "stack the cups",
} satisfies LeaseStatus;

test("a stale snapshot of the viewer's own lease does not block reacquisition", () => {
  expect(manualControlView(ownLease, false, ownLabel)).toEqual({
    kind: "available",
  });
  expect(manualControlView(null, false, ownLabel)).toEqual({
    kind: "available",
  });
});

test("manual motion requires both the local token and current station ownership", () => {
  const holding = manualControlView(ownLease, true, ownLabel);
  expect(holding).toEqual({ kind: "holding" });
  expect(manualControlBlockedReason(holding)).toBeNull();

  const lapsed = manualControlView(null, true, ownLabel);
  expect(lapsed).toEqual({ kind: "lapsed" });
  expect(manualControlBlockedReason(lapsed)).toContain(
    "confirm manual control",
  );
  expect(
    manualControlBlockedReason(manualControlView(null, false, ownLabel)),
  ).toBe("Reconnect manual control to move the arm.");
  for (const holder of [otherViewer, agentLease]) {
    for (const hasToken of [true, false]) {
      const blocked = manualControlView(holder, hasToken, ownLabel);
      expect(blocked).toEqual({ kind: "elsewhere", heldBy: holder });
      expect(manualControlBlockedReason(blocked)).toContain(holder.label);
    }
  }
});

test("a lease refusal is distinct from an arm refusal", () => {
  const refusal = (
    reason: Extract<CommandAnswer, { status: "refused" }>["reason"],
  ): CommandAnswer => ({
    status: "refused",
    reason,
    detail: "Command not admitted",
  });
  expect(isLeaseHeldElsewhere(refusal("lease_held_elsewhere"))).toBe(true);
  for (const reason of ["not_enabled", "arm_busy", "unreachable"] as const)
    expect(isLeaseHeldElsewhere(refusal(reason))).toBe(false);
});
