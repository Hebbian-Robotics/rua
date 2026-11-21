import type {
  CommandAnswer,
  CommandRecord,
  StationState,
} from "@hebbian/robot-use-client";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { describeCommandOutcome } from "../../command-outcome.js";
import { describeLease, manualControlView } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { armControl, manualLease } from "../../manual-lease.js";
import { ConfirmationDialog } from "../../shared/confirmation-dialog.js";
import { stationConnection } from "../../viewer-connection.js";
import { InterlockStatus } from "./interlock-status.js";

function completedArmCommand(
  answer: CommandAnswer,
  armName: string,
): CommandRecord {
  if (answer.status === "refused") throw new Error(answer.detail);
  const record = answer.records.find((entry) => entry.arm === armName)?.record;
  if (!record)
    throw new Error("The station did not return an arm command record.");
  if (record.completion?.outcome !== "completed")
    throw new Error(
      record.completion?.detail ?? "The command did not complete.",
    );
  if (record.path_limited !== null || record.pose_limited !== null)
    throw new Error("The arm could not reach the requested home pose.");
  return record;
}

async function waitForStationState(
  predicate: (state: StationState) => boolean,
): Promise<void> {
  const current = stationConnection.getState();
  if (current && predicate(current)) return;
  await new Promise<void>((resolve, reject) => {
    const deadline = window.setTimeout(() => {
      finish(() =>
        reject(new Error("Timed out waiting for the arm or lease state.")),
      );
    }, 10000);
    const unsubscribe = stationConnection.onState((state) => {
      if (predicate(state)) finish(resolve);
    });
    function finish(settle: () => void): void {
      window.clearTimeout(deadline);
      unsubscribe();
      settle();
    }
  });
}

async function stopAndWaitForArms(armNames: string[]): Promise<void> {
  for (const armName of armNames) {
    const answer = await armControl(armName).stop();
    if (answer.status === "refused")
      throw new Error(`${armName}: ${answer.detail}`);
    const stopRecord = answer.records.find(
      (item) => item.arm === armName,
    )?.record;
    if (!stopRecord)
      throw new Error(`${armName}: no stop command record returned.`);
    if (stopRecord.completion?.outcome !== "completed")
      throw new Error(
        `${armName}: ${stopRecord.completion?.detail ?? "Stop was not accepted."}`,
      );
    await waitForStationState((state) =>
      state.arms.some(
        (arm) =>
          arm.arm === armName &&
          arm.mode === "holding" &&
          arm.active_command === null &&
          arm.sampled_at_unix_seconds !== null &&
          arm.sampled_at_unix_seconds >= stopRecord.accepted_at,
      ),
    );
  }
}

export function ArmControl({
  stationState,
  armName,
  onSelectArm,
  runActive,
  onStopFollowing,
}: {
  stationState: StationState | null;
  armName: string;
  onSelectArm: (name: string) => void;
  runActive: boolean;
  onStopFollowing: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState("");
  const [rehoming, setRehoming] = useState(false);
  const rehomeCancellation = useRef<AbortController | null>(null);
  useEffect(() => () => rehomeCancellation.current?.abort(), []);
  const busy = pending || rehoming;
  const arm = stationState?.arms.find((candidate) => candidate.arm === armName);
  const lastOutcome = arm?.last_command
    ? describeCommandOutcome(arm.last_command)
    : null;
  const leaseView = manualControlView(
    stationState?.lease ?? null,
    manualLease.holdsLease(),
    manualLease.label,
  );
  const canRehome =
    arm?.mode === "holding" &&
    arm.rest_pose !== null &&
    arm.interlocks.length === 0 &&
    !busy &&
    !runActive &&
    leaseView.kind !== "elsewhere";
  const canDisableInPlace =
    arm !== undefined &&
    !busy &&
    !runActive &&
    !["disabled", "reconnected", "disconnected"].includes(arm.mode);

  async function rehomeArm(disableAfterRehome: boolean): Promise<void> {
    if (!arm || !canRehome || rehomeCancellation.current !== null) return;
    onStopFollowing();
    const cancellation = new AbortController();
    rehomeCancellation.current = cancellation;
    setRehoming(true);
    setMessage(disableAfterRehome ? "Rehoming, then disabling…" : "Rehoming…");
    let acquiredLease = false;
    try {
      if (!manualLease.holdsLease()) {
        const outcome = await manualLease.acquire(false);
        if (outcome.kind === "failed") throw new Error(outcome.message);
        if (outcome.kind === "refused")
          throw new Error(
            "Another client holds the control lease. Take manual control first.",
          );
        acquiredLease = true;
      }
      cancellation.signal.throwIfAborted();
      completedArmCommand(
        await manualLease.rehome(arm.arm, cancellation.signal),
        arm.arm,
      );
      cancellation.signal.throwIfAborted();
      if (disableAfterRehome) {
        completedArmCommand(
          await stationConnection
            .arm(arm.arm, { signal: cancellation.signal })
            .disable(),
          arm.arm,
        );
      }
      setMessage(
        disableAfterRehome
          ? "Arm rehomed and disabled."
          : "Arm rehomed and holding.",
      );
    } catch (error: unknown) {
      setMessage(
        cancellation.signal.aborted
          ? "Rehome cancelled."
          : `Could not rehome the arm: ${errorMessage(error)}`,
      );
    } finally {
      if (acquiredLease) void manualLease.returnLease();
      rehomeCancellation.current = null;
      setRehoming(false);
    }
  }

  async function changePower(kind: "enable" | "disable"): Promise<void> {
    if (!arm || busy) return;
    setPending(true);
    setMessage("");
    try {
      if (kind === "disable") onStopFollowing();
      const answer =
        kind === "enable"
          ? await armControl(arm.arm).enable()
          : await armControl(arm.arm).disable();
      if (answer.status === "refused") throw new Error(answer.detail);
      const commandId = answer.records.find((item) => item.arm === arm.arm)
        ?.record.command_id;
      if (!commandId)
        throw new Error("The station did not return an arm command record.");
      await waitForStationState((state) =>
        state.arms.some(
          (item) =>
            item.arm === arm.arm &&
            item.last_command?.command_id === commandId &&
            item.last_command.completion !== null,
        ),
      );
      const completion = stationConnection
        .getState()
        ?.arms.find((item) => item.arm === arm.arm)?.last_command?.completion;
      if (completion?.outcome !== "completed")
        throw new Error(completion?.detail ?? `Arm ${kind} did not complete.`);
      setMessage(
        kind === "enable" ? "Arm enabled and holding." : "Arm disabled.",
      );
    } catch (error: unknown) {
      setMessage(
        `Could not ${kind === "enable" ? "enable" : "disable"} the arm: ${errorMessage(error)}`,
      );
    } finally {
      setPending(false);
    }
  }
  async function stopArm(): Promise<void> {
    if (!arm || pending) return;
    rehomeCancellation.current?.abort();
    onStopFollowing();
    setPending(true);
    try {
      await stopAndWaitForArms([arm.arm]);
      setMessage("Arm stopped and holding.");
    } catch (error: unknown) {
      setMessage(`Could not stop the arm: ${errorMessage(error)}`);
    } finally {
      setPending(false);
    }
  }
  return (
    <section className="arm-control" aria-label="Arm status and power">
      <div className="arm-control-heading">
        <h3>Arm</h3>
        <span>
          {stationState
            ? describeLease(stationState.lease, manualLease.label)
            : "Station offline"}
        </span>
      </div>
      <label htmlFor="selected-arm">
        Selected arm
        <NativeSelect
          id="selected-arm"
          value={armName}
          disabled={!stationState || busy || runActive}
          onChange={(event) => onSelectArm(event.currentTarget.value)}
        >
          {stationState?.arms.map((item) => (
            <NativeSelectOption key={item.arm} value={item.arm}>
              {item.arm}
            </NativeSelectOption>
          ))}
        </NativeSelect>
      </label>
      <p>
        {arm
          ? `${arm.arm}: ${arm.mode}${arm.fault || arm.active_trip ? ` — ${arm.fault || arm.active_trip}` : ""}`
          : "Waiting for arm state."}
      </p>
      {arm && <InterlockStatus arm={arm} onMessage={setMessage} />}
      {lastOutcome && <p>{lastOutcome.text}</p>}
      <div className="control-actions">
        <Button
          disabled={
            !arm ||
            busy ||
            runActive ||
            !["disabled", "reconnected"].includes(arm.mode)
          }
          onClick={() => void changePower("enable")}
        >
          Enable arm
        </Button>
        <Button
          variant="destructive"
          disabled={
            !arm || pending || !["holding", "moving"].includes(arm.mode)
          }
          onClick={() => void stopArm()}
        >
          Stop and hold
        </Button>
        <Button disabled={!canRehome} onClick={() => void rehomeArm(true)}>
          Rehome + Disable
        </Button>
        <Button
          variant="outline"
          disabled={!canRehome}
          onClick={() => void rehomeArm(false)}
        >
          Rehome
        </Button>
      </div>
      <p>
        Rehome returns to the pose saved when the arm was enabled. Use Rehome +
        Disable when the saved pose lets the arm rest securely.
      </p>
      <div className="arm-control-direct-disable">
        <ConfirmationDialog
          trigger={
            <Button variant="outline" size="sm" disabled={!canDisableInPlace}>
              Disable in place…
            </Button>
          }
          title={`Disable ${armName} in place?`}
          description="Removing motor torque can cause the arm to drop. Physically support the arm before confirming."
          actionLabel="Disable arm"
          actionDisabled={!canDisableInPlace}
          onConfirm={() => {
            if (canDisableInPlace) void changePower("disable");
          }}
        />
      </div>
      {message && <p role="status">{message}</p>}
    </section>
  );
}
