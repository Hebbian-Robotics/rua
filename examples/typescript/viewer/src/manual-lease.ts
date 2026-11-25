import {
  armMotionCommands,
  type CommandAnswer,
  type Lease,
  LeaseHeldElsewhereError,
  type StationCommand,
} from "hebbian-rua";
import {
  type AcquireOutcome,
  interpretLeaseAnswer,
  isLeaseHeldElsewhere,
  newManualLabel,
} from "./control-logic.js";
import { errorMessage } from "./errors.js";
import { stationConnection, type ViewerLease } from "./viewer-connection.js";

export interface ManualLease {
  readonly label: string;
  holdsLease(): boolean;

  acquire(): Promise<AcquireOutcome>;

  returnLease(): Promise<void>;

  send(
    arm: string,
    command: StationCommand,
    signal?: AbortSignal,
    operationId?: string,
  ): Promise<CommandAnswer>;

  updateStreamGoal: Lease["updateStreamGoal"];

  rehome(arm: string, signal: AbortSignal): Promise<CommandAnswer>;

  noteCommandAnswer(answer: CommandAnswer): void;

  onChange(listener: () => void): () => void;
}

export function createManualLease(
  connection: Pick<typeof stationConnection, "acquireLease" | "onLeaseLost">,
  randomHex: () => string = () =>
    Math.floor(Math.random() * 0x10000)
      .toString(16)
      .padStart(4, "0"),
): ManualLease {
  const label = newManualLabel(randomHex());
  let held = false;
  let current: ViewerLease | null = null;
  let returningLease: Promise<void> | null = null;
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
    async acquire(): Promise<AcquireOutcome> {
      try {
        await returningLease;
        const lease = await connection.acquireLease(label);
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
    returnLease(): Promise<void> {
      if (returningLease) return returningLease;
      const lease = current;
      if (lease === null) return Promise.resolve();
      returningLease = lease
        .return()
        .catch((error: unknown) => {
          console.warn("Could not return manual lease", error);
        })
        .finally(() => {
          returningLease = null;
        });
      setHeld(false);
      return returningLease;
    },
    async send(
      arm: string,
      command: StationCommand,
      signal?: AbortSignal,
      operationId?: string,
    ): Promise<CommandAnswer> {
      if (current === null) throw new Error("Manual control is not connected");
      const answer = await current.sendCommand(
        arm,
        command,
        undefined,
        undefined,
        signal,
        operationId,
      );
      if (isLeaseHeldElsewhere(answer)) setHeld(false);
      return answer;
    },
    async updateStreamGoal(operationId, goal, signal) {
      if (current === null) throw new Error("Manual control is not connected");
      const answer = await current.updateStreamGoal(operationId, goal, signal);
      if (isLeaseHeldElsewhere(answer)) setHeld(false);
      return answer;
    },
    async rehome(arm: string, signal: AbortSignal): Promise<CommandAnswer> {
      if (current === null) throw new Error("Manual control is not connected");
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

export function armMotion(
  arm: string,
  signal?: AbortSignal,
  operationId?: string,
) {
  return armMotionCommands((command) =>
    manualLease.send(arm, command, signal, operationId),
  );
}

export function armControl(arm: string) {
  return stationConnection.arm(arm, { waitForOutcome: false });
}
