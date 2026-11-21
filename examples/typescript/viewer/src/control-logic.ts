// What the viewer may do given who holds the lease on the arms. Pure: no DOM, no
// network, so every decision here is tested directly.
//
// Only the station's own answers say a lease is gone (lease_lost, a closed
// connection, or a lease_held_elsewhere refusal). A state snapshot can predate
// an acquire, so it is never used to drop a lease, only to decide what to show
// and allow.
//
// The station arbitrates the lease: one holder at a time, held by a connection,
// and `stop` accepted from anyone. These functions only interpret what it reports.

import type {
  CommandAnswer,
  Interlock,
  LeaseAnswer,
  LeaseStatus,
} from "hebbian-rua";
import { match } from "ts-pattern";

export function newManualLabel(randomHex: string): string {
  return `viewer ${randomHex}`;
}

export type ManualControlView =
  // No one holds the lease: the user may turn manual control on.
  | { kind: "available" }
  // This viewer holds the lease: manual controls are live.
  | { kind: "holding" }
  // This viewer believed it held the lease but the station shows no holder:
  // the lease lapsed.
  | { kind: "lapsed" }
  // Someone else holds the lease, or took it from this viewer.
  | { kind: "elsewhere"; heldBy: LeaseStatus };

export function manualControlView(
  stationLease: LeaseStatus | null,
  holdsToken: boolean,
  ownLabel: string,
): ManualControlView {
  if (stationLease === null)
    return holdsToken ? { kind: "lapsed" } : { kind: "available" };
  const namesThisViewer =
    stationLease.holder === "manual" && stationLease.label === ownLabel;
  if (namesThisViewer)
    // Without the lease the report is a leftover of this viewer's own lease (it
    // just returned it, and the state stream has not caught up): nobody else.
    return holdsToken ? { kind: "holding" } : { kind: "available" };
  return { kind: "elsewhere", heldBy: stationLease };
}

export function manualControlBlockedReason(
  view: ManualControlView,
): string | null {
  return match(view)
    .with({ kind: "holding" }, () => null)
    .with(
      { kind: "available" },
      () => "Reconnect manual control to move the arm.",
    )
    .with(
      { kind: "lapsed" },
      () => "Waiting for the station to confirm manual control.",
    )
    .with(
      { kind: "elsewhere" },
      ({ heldBy }) => `${describeHolder(heldBy)} holds control.`,
    )
    .exhaustive();
}

export function describeHolder(lease: LeaseStatus): string {
  return match(lease.holder)
    .with("agent", () => `Agent "${lease.label}"`)
    .with("manual", () => `Manual control (${lease.label})`)
    .exhaustive();
}

export function describeLease(
  lease: LeaseStatus | null,
  ownLabel: string,
): string {
  if (lease === null) return "No one holds the lease";
  if (lease.holder === "manual" && lease.label === ownLabel)
    return "Manual control: this viewer holds the lease";
  return `${describeHolder(lease)} holds the lease`;
}

export type AcquireOutcome =
  | { kind: "granted"; leaseSeconds: number }
  | { kind: "refused"; heldBy: LeaseStatus }
  | { kind: "failed"; message: string };

export function interpretLeaseAnswer(answer: LeaseAnswer): AcquireOutcome {
  if (answer.kind === "granted")
    return { kind: "granted", leaseSeconds: answer.lease_seconds };
  return answer.status
    ? { kind: "refused", heldBy: answer.status }
    : { kind: "failed", message: "The lease was not granted" };
}

export function isLeaseHeldElsewhere(answer: CommandAnswer): boolean {
  return (
    answer.status === "refused" && answer.reason === "lease_held_elsewhere"
  );
}

export function describeInterlock(interlock: Interlock): string {
  return match(interlock)
    .with({ kind: "arm_disconnected" }, () => "arm disconnected")
    .with(
      { kind: "recovery_approval_required" },
      ({ motors }) =>
        `recovery approval required${motors.length ? ` (motors ${motors.join(", ")} without torque)` : ""}`,
    )
    .with(
      { kind: "required_camera_unavailable" },
      ({ camera }) => `required camera ${camera} unavailable`,
    )
    .exhaustive();
}

export type CommandResult =
  | { kind: "accepted" }
  | { kind: "closest_pose"; advice: string }
  | { kind: "rejected"; message: string };

export function commandMessage(result: CommandResult): string {
  return match(result)
    .with(
      { kind: "accepted" },
      () => "Command accepted. See arm status for the outcome.",
    )
    .with({ kind: "closest_pose" }, ({ advice }) => advice)
    .with({ kind: "rejected" }, ({ message }) => `Command rejected: ${message}`)
    .exhaustive();
}
