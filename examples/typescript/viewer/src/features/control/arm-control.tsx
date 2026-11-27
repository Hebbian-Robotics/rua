import {
  type CommandAnswer,
  type CommandRecord,
  commandResult,
  commandWasLimited,
  type StationState,
} from "hebbian-rua";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { describeCommandOutcome } from "../../command-outcome.js";
import { describeHolder, manualControlView } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { armControl, manualLease } from "../../manual-lease.js";
import { ConfirmationDialog } from "../../shared/confirmation-dialog.js";
import {
  formatMotionDetail,
  isJointTargetOutsideLimits,
} from "../../shared/measurements.js";
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
  const completion = commandResult(record);
  if (completion?.outcome !== "completed")
    throw new Error(completion?.detail ?? "The command did not complete.");
  if (commandWasLimited(record))
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
    const completion = commandResult(stopRecord);
    if (completion?.outcome !== "completed")
      throw new Error(
        `${armName}: ${completion?.detail ?? "Stop was not accepted."}`,
      );
    await waitForStationState((state) =>
      state.arms.some(
        (arm) =>
          arm.arm === armName &&
          arm.mode === "holding" &&
          arm.active_command === null &&
          arm.sampled_unix_nano !== null &&
          BigInt(arm.sampled_unix_nano) >=
            BigInt(stopRecord.accepted_unix_nano),
      ),
    );
  }
}

export function ArmControl({
  stationState,
  armName,
  onSelectArm,
  runActive,
  manualSelected,
  onManualControlStarted,
  onStopFollowing,
  onPowerTransition,
}: {
  stationState: StationState | null;
  armName: string;
  onSelectArm: (name: string) => void;
  runActive: boolean;
  manualSelected: boolean;
  onManualControlStarted?: () => Promise<void>;
  onStopFollowing: () => void;
  onPowerTransition: (changing: boolean) => void;
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
  const holdsLease = useSyncExternalStore(
    manualLease.onChange,
    manualLease.holdsLease,
  );
  const leaseView = manualControlView(
    stationState?.lease ?? null,
    holdsLease,
    manualLease.label,
  );
  const powerOff = arm?.mode === "disabled" || arm?.mode === "reconnected";
  const canReconnectManual =
    manualSelected &&
    arm?.mode === "holding" &&
    !holdsLease &&
    leaseView.kind !== "elsewhere";
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

  async function acquireManualLease(): Promise<void> {
    const outcome = await manualLease.acquire();
    if (outcome.kind === "failed") throw new Error(outcome.message);
    if (outcome.kind === "refused")
      throw new Error(`${describeHolder(outcome.heldBy)} holds control.`);
  }

  async function rehomeArm(disableAfterRehome: boolean): Promise<void> {
    if (!arm || !canRehome || rehomeCancellation.current !== null) return;
    onStopFollowing();
    const cancellation = new AbortController();
    rehomeCancellation.current = cancellation;
    setRehoming(true);
    if (disableAfterRehome) onPowerTransition(true);
    setMessage(disableAfterRehome ? "Rehoming, then disabling…" : "Rehoming…");
    let acquiredLease = false;
    try {
      if (!manualLease.holdsLease()) {
        await acquireManualLease();
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
      const detail = errorMessage(error);
      setMessage(
        cancellation.signal.aborted
          ? "Rehome cancelled."
          : isJointTargetOutsideLimits(detail)
            ? `The saved home pose is outside the joint limits. ${detail} Enable the arm from a pose within its limits to save a new home pose.`
            : `Could not rehome the arm: ${detail}`,
      );
    } finally {
      if (acquiredLease) await manualLease.returnLease();
      rehomeCancellation.current = null;
      setRehoming(false);
      if (disableAfterRehome) onPowerTransition(false);
    }
  }

  async function changePower(kind: "enable" | "disable"): Promise<void> {
    if (!arm || busy) return;
    setPending(true);
    onPowerTransition(true);
    setMessage("");
    let acquiredLease = false;
    try {
      if (kind === "disable") onStopFollowing();
      if (kind === "enable" && manualSelected && !manualLease.holdsLease()) {
        await acquireManualLease();
        acquiredLease = true;
      }
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
            item.last_command.phase.kind !== "pending",
        ),
      );
      const completion = commandResult(
        stationConnection.getState()?.arms.find((item) => item.arm === arm.arm)
          ?.last_command,
      );
      if (completion?.outcome !== "completed")
        throw new Error(completion?.detail ?? `Arm ${kind} did not complete.`);
      if (kind === "enable" && manualSelected && onManualControlStarted)
        await onManualControlStarted();
    } catch (error: unknown) {
      if (acquiredLease) await manualLease.returnLease();
      setMessage(
        `Could not ${kind === "enable" ? "enable" : "disable"} the arm: ${errorMessage(error)}`,
      );
    } finally {
      setPending(false);
      onPowerTransition(false);
    }
  }
  async function reconnectManual(): Promise<void> {
    if (!canReconnectManual || busy || runActive) return;
    setPending(true);
    setMessage("");
    try {
      await acquireManualLease();
      if (onManualControlStarted) await onManualControlStarted();
    } catch (error: unknown) {
      setMessage(`Could not reconnect manual control: ${errorMessage(error)}`);
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
        <h3>{arm?.arm ?? "Arm"}</h3>
        {arm && <span>{arm.mode}</span>}
      </div>
      {(stationState?.arms.length ?? 0) > 1 && (
        <label htmlFor="selected-arm">
          Selected arm
          <NativeSelect
            id="selected-arm"
            value={armName}
            disabled={busy || runActive}
            onChange={(event) => onSelectArm(event.currentTarget.value)}
          >
            {stationState?.arms.map((item) => (
              <NativeSelectOption key={item.arm} value={item.arm}>
                {item.arm}
              </NativeSelectOption>
            ))}
          </NativeSelect>
        </label>
      )}
      {arm && <InterlockStatus arm={arm} onMessage={setMessage} />}
      {lastOutcome?.text &&
        (lastOutcome.isError ||
          commandResult(arm?.last_command)?.outcome === "stopped_short") &&
        !arm?.fault &&
        !arm?.active_trip && <p>{lastOutcome.text}</p>}
      {powerOff && (
        <Button
          id="enable-arm"
          className="w-fit"
          disabled={
            busy ||
            runActive ||
            (manualSelected && leaseView.kind === "elsewhere")
          }
          onClick={() => void changePower("enable")}
        >
          Enable arm
        </Button>
      )}
      {manualSelected && leaseView.kind === "elsewhere" && (
        <p role="status">{describeHolder(leaseView.heldBy)} holds control.</p>
      )}
      {arm &&
        !["disabled", "reconnected", "disconnected"].includes(arm.mode) && (
          <>
            <div className="control-actions">
              <Button
                variant="stop"
                disabled={pending || !["holding", "moving"].includes(arm.mode)}
                onClick={() => void stopArm()}
              >
                Stop and hold
              </Button>
              <Button
                disabled={!canRehome}
                onClick={() => void rehomeArm(true)}
              >
                Rehome and disable
              </Button>
              <Button
                variant="outline"
                disabled={!canRehome}
                onClick={() => void rehomeArm(false)}
              >
                Rehome
              </Button>
            </div>
            {canReconnectManual && (
              <Button
                variant="outline"
                className="w-fit"
                disabled={busy || runActive}
                onClick={() => void reconnectManual()}
              >
                Reconnect manual control
              </Button>
            )}
            <div className="arm-control-direct-disable">
              <ConfirmationDialog
                trigger={
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={!canDisableInPlace}
                    aria-describedby="disable-in-place-note"
                  >
                    Disable in place…
                  </Button>
                }
                title={`Disable ${armName} in place?`}
                description="This can make it drop. Support the arm before continuing."
                actionLabel="Disable arm"
                actionDisabled={!canDisableInPlace}
                onConfirm={() => {
                  if (canDisableInPlace) void changePower("disable");
                }}
              />
              <p id="disable-in-place-note">This can make it drop.</p>
            </div>
          </>
        )}
      <p className="empty:hidden" role="status">
        {formatMotionDetail(message)}
      </p>
    </section>
  );
}
