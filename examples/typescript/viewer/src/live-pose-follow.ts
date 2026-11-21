import type { ArmState } from "@hebbian/robot-use-client";
import { createActor } from "xstate";
import { errorMessage } from "./errors.js";
import { createLatestOnly } from "./lib/latest-only.js";
import { createLivePoseFollowMachine } from "./live-pose-follow-machine.js";
import { armControl, armMotion } from "./manual-lease.js";

export type FollowArm = Pick<ArmState, "arm" | "mode"> & {
  sampled_at_unix_seconds?: number | null;
};

// Hand-tuned for the manual-follow feel; not derived from a standard.
const defaultJointSpeedRadiansPerSecond = 0.08;
const STALE_FEEDBACK_SECONDS = 0.5;
// The station ends a stream that gets no goal for `stream_goal_timeout_seconds`
// (0.5 s). A held drag has nothing new to say once the arm catches up, so the
// newest goal is repeated well inside that deadline to keep the stream open.
const GOAL_KEEPALIVE_MILLISECONDS = 200;
// Refusals after which a later goal can still succeed: the stream ended or
// tripped (stopping at the table, a joint limit), or this goal cannot be had.
const GOAL_REFUSALS_FOLLOW_SURVIVES: ReadonlySet<string> = new Set([
  "stream_ended",
  "invalid_target",
  "unreachable",
]);

export function createLivePoseFollow(
  getArm: () => FollowArm | undefined,
  report: (message: string) => void,
  getJointSpeed: () => number = () => defaultJointSpeedRadiansPerSecond,
  onDisarmed: () => void = () => {},
) {
  const lifecycle = createActor(
    createLivePoseFollowMachine(stopMotion),
  ).start();
  let streamId: string | null = null;
  let streamingArm: string | null = null;
  let newestGoal: { arm: string; joints: number[] } | null = null;
  let keepAlive: ReturnType<typeof setInterval> | null = null;
  // INVARIANT: Bumped when streaming ends: an answer from an older epoch is ignored.
  let epoch = 0;

  const isFollowing = () =>
    lifecycle.getSnapshot().matches({ control: "armed", pointer: "dragging" });
  const isStreaming = () =>
    lifecycle.getSnapshot().matches({ stream: "active" });

  const latestGoal = createLatestOnly<{ arm: string; joints: number[] }>(
    async (goal) => {
      const arm = getArm();
      if (!isFollowing() || !arm) return;
      if (goal.arm !== arm.arm) {
        disarm("Follow stopped: the selected arm changed.");
        return;
      }
      if (streamingArm !== null && streamingArm !== arm.arm) {
        disarm("Follow stopped: the selected arm changed.");
        return;
      }
      const mode = arm.mode;
      if (mode !== "holding" && !(isStreaming() && mode === "moving")) return;
      if (
        arm.sampled_at_unix_seconds != null &&
        Date.now() / 1000 - arm.sampled_at_unix_seconds > STALE_FEEDBACK_SECONDS
      ) {
        disarm("Follow stopped: robot feedback is stale.");
        return;
      }
      const sentInEpoch = epoch;
      streamingArm = arm.arm;
      lifecycle.send({ type: "STREAM_START" });
      const answer = await armMotion(arm.arm)
        .streamJoints(
          goal.joints,
          undefined,
          getJointSpeed(),
          streamId ?? undefined,
        )
        .catch((error: unknown) => {
          if (sentInEpoch !== epoch) return null;
          throw error;
        });
      if (sentInEpoch !== epoch || answer === null) return;
      if (answer.status === "refused") {
        // The goal or the stream ended, but follow itself can go on: the next
        // goal the drag produces starts a new stream. Anything else (a lost
        // lease, an interlock, a disabled arm) is not the drag's to retry.
        if (!GOAL_REFUSALS_FOLLOW_SURVIVES.has(answer.reason))
          throw new Error(answer.detail);
        endStream();
        report(`Follow paused: ${answer.detail}. Keep dragging to continue.`);
        return;
      }
      const record = answer.records[0]?.record;
      if (!record) throw new Error("The station accepted no stream");
      streamId = record.command_id;
      keepAlive ??= setInterval(() => {
        if (isFollowing() && newestGoal) latestGoal.push(newestGoal);
      }, GOAL_KEEPALIVE_MILLISECONDS);
      report(
        "Following the reachable preview. Release the drag to stop and hold.",
      );
    },
    (error) => disarm(`Follow stopped: ${errorMessage(error)}`),
  );

  function endStream(): void {
    if (keepAlive !== null) clearInterval(keepAlive);
    keepAlive = null;
    lifecycle.send({ type: "STREAM_END" });
    streamId = null;
    streamingArm = null;
  }

  function stopMotion(): void {
    epoch += 1;
    if (keepAlive !== null) clearInterval(keepAlive);
    keepAlive = null;
    latestGoal.cancel();
    if (!isStreaming()) return;
    const stoppedArm = streamingArm;
    lifecycle.send({ type: "STREAM_END" });
    streamId = null;
    streamingArm = null;
    // A request still on the wire may be accepted late, so stop once it has
    // landed (unless a new drag started streaming again).
    const stopArm = async (): Promise<void> => {
      if (!stoppedArm || streamingArm === stoppedArm) return;
      try {
        await armControl(stoppedArm).stop();
      } catch {
        report(
          "Connection lost. The station will stop the stream when goals expire.",
        );
      }
    };
    void latestGoal.idle().then(stopArm);
  }

  function disarm(reason: string): void {
    lifecycle.send({ type: "DISABLE" });
    report(reason);
    onDisarmed();
  }

  function suspend(): void {
    if (lifecycle.getSnapshot().matches({ control: "armed" }))
      disarm("Follow stopped: the viewer lost focus.");
    lifecycle.send({ type: "DRAG_END" });
  }
  window.addEventListener("blur", suspend);
  function onVisibilityChange(): void {
    if (document.hidden) suspend();
  }
  document.addEventListener("visibilitychange", onVisibilityChange);

  return {
    destroy(): void {
      window.removeEventListener("blur", suspend);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      lifecycle.send({ type: "STOP" });
      lifecycle.stop();
    },
    setEnabled(value: boolean): void {
      lifecycle.send({ type: value ? "ENABLE" : "DISABLE" });
      if (value)
        report(
          "Follow mode armed. Drag a reachable target to move; release to stop.",
        );
    },
    setDragging(value: boolean): void {
      lifecycle.send({ type: value ? "DRAG_START" : "DRAG_END" });
    },
    setGoal(joints: number[] | null): void {
      const arm = getArm();
      newestGoal = joints && arm ? { arm: arm.arm, joints } : null;
      if (!newestGoal) {
        lifecycle.send({ type: "STOP" });
        return;
      }
      if (isFollowing()) latestGoal.push(newestGoal);
    },
    disarm,
    stop: () => lifecycle.send({ type: "STOP" }),
  };
}
