// This viewer's manual lease on the arms: acquire, take, return, and the
// commands that go out under it. The rules about what the lease allows are in
// control-logic.ts; this holds the state and talks to the executor.
//
// The SDK renews the HTTP lease while its event stream is healthy. The station
// stops leased motion when the lease expires. Commands that move an arm go through the lease
// handle; enable, disable, clear_fault, approve_recovery and stop do not need it. Nothing here touches
// `window` or `document` when imported: the commands that import it are tested
// under a fake browser.

import {
  armMotionCommands,
  type CommandAnswer,
  type Lease,
  LeaseHeldElsewhereError,
  type StationCommand,
  type StationConnection,
} from "@hebbian/robot-use-client";
import {
  type AcquireOutcome,
  interpretLeaseAnswer,
  isLeaseHeldElsewhere,
  newManualLabel,
} from "./control-logic.js";
import { errorMessage } from "./errors.js";
import { stationConnection } from "./viewer-connection.js";

export interface ManualLease {
  readonly label: string;
  holdsLease(): boolean;

  acquire(take: boolean): Promise<AcquireOutcome>;

  returnLease(): Promise<void>;

  send(arm: string, command: StationCommand): Promise<CommandAnswer>;

  rehome(arm: string, signal: AbortSignal): Promise<CommandAnswer>;

  noteCommandAnswer(answer: CommandAnswer): void;

  onChange(listener: () => void): () => void;
}

export function createManualLease(
  connection: Pick<
    StationConnection,
    "acquireLease" | "takeLease" | "onLeaseLost"
  >,
  randomHex: () => string = () =>
    Math.floor(Math.random() * 0x10000)
      .toString(16)
      .padStart(4, "0"),
): ManualLease {
  const label = newManualLabel(randomHex());
  let held = false;
  let current: Lease | null = null;
  const listeners = new Set<() => void>();

  function setHeld(nowHeld: boolean): void {
    if (!nowHeld) current = null;
    if (held === nowHeld) return;
    held = nowHeld;
    for (const listener of listeners) listener();
  }
  connection.onLeaseLost(() => setHeld(false));

  return {
    label,
    holdsLease: () => held,
    async acquire(take: boolean): Promise<AcquireOutcome> {
      try {
        const lease = await (take
          ? connection.takeLease(label)
          : connection.acquireLease(label));
        current = lease;
        setHeld(true);
        return { kind: "granted", leaseSeconds: lease.leaseSeconds };
      } catch (error: unknown) {
        if (error instanceof LeaseHeldElsewhereError)
          return interpretLeaseAnswer(error.answer);
        return {
          kind: "failed",
          message: `Could not reach the station: ${errorMessage(error)}`,
        };
      }
    },
    async returnLease(): Promise<void> {
      const lease = current;
      if (!held || lease === null) return;
      setHeld(false);
      await lease.return().catch((error: unknown) => {
        console.warn("Could not return manual lease", error);
      });
    },
    async send(arm: string, command: StationCommand): Promise<CommandAnswer> {
      if (current === null)
        throw new Error("Manual control is not held; take it first");
      const answer = await current.sendCommand(arm, command);
      if (isLeaseHeldElsewhere(answer)) setHeld(false);
      return answer;
    },
    async rehome(arm: string, signal: AbortSignal): Promise<CommandAnswer> {
      if (current === null)
        throw new Error("Manual control is not held; take it first");
      const answer = await current.arm(arm, { signal }).rehome();
      if (isLeaseHeldElsewhere(answer)) setHeld(false);
      return answer;
    },
    noteCommandAnswer(answer: CommandAnswer): void {
      if (isLeaseHeldElsewhere(answer)) setHeld(false);
    },
    onChange(listener: () => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

export const manualLease: ManualLease = createManualLease(stationConnection);

export function sendArmCommand(
  arm: string,
  command: StationCommand,
  lease: ManualLease = manualLease,
): Promise<CommandAnswer> {
  return lease.send(arm, command);
}

export function armMotion(arm: string) {
  return armMotionCommands((command) => sendArmCommand(arm, command));
}

export function armControl(arm: string) {
  return stationConnection.arm(arm, { waitForOutcome: false });
}
