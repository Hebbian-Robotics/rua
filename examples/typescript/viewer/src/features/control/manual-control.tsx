import {
  type ArmState,
  angularDegrees,
  angularRadians,
  commandPlan,
  type PreviewPose,
  type StationCommand,
  type StationState,
} from "hebbian-rua";
import {
  type RefObject,
  useCallback,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { MathUtils } from "three";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Switch } from "@/components/ui/switch.js";
import type { MotionLimits } from "../../../shared/run-api.js";
import {
  type CommandResult,
  commandMessage,
  manualControlBlockedReason,
  manualControlView,
} from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { createLiveGripper } from "../../live-gripper.js";
import { createLivePoseFollow } from "../../live-pose-follow.js";
import { manualLease, sendArmCommand } from "../../manual-lease.js";
import {
  type Orientation,
  type ProposedPose,
  previewRequest,
  readCurrentOrientation,
} from "../../pose-projection.js";
import {
  describeApproach,
  moveCommandForPreview,
  movePreviewedGripper,
  syncCurrentPosePreview,
  unreachableYawLines,
} from "../../preview-move.js";
import {
  formatMotionDetail,
  metersToMillimeters,
  millimetersToMeters,
} from "../../shared/measurements.js";
import { stationConnection } from "../../viewer-connection.js";
import type { SceneProjection } from "../robot-scene/robot-scene.js";
import {
  armAcceptsManualControl,
  manualMoveBlockedReason,
  previewMatchesSelection,
} from "./manual-control-logic.js";
import {
  createProjectionController,
  interpretProjection,
} from "./projection-controller.js";

type Position = [number, number, number];
type ManualProps = {
  stationState: StationState | null;
  motionLimits: MotionLimits;
  armName: string;
  changingArmPower: boolean;
  active: boolean;
  target: Position | null;
  setTarget: (target: Position | null) => void;
  projection: SceneProjection | null;
  setProjection: (projection: SceneProjection | null) => void;
  projectionValidity: boolean | null;
  dragging: boolean;
  globalStopHandler: RefObject<(() => void) | null>;
};

type Follow = ReturnType<typeof createLivePoseFollow>;
type Gripper = ReturnType<typeof createLiveGripper>;
type ProjectionController = ReturnType<typeof createProjectionController>;

async function requestManualCommand(
  arm: string,
  command: StationCommand,
): Promise<CommandResult> {
  try {
    const answer = await sendArmCommand(arm, command);
    if (answer.status === "refused")
      return { kind: "rejected", message: answer.detail };
    const limited = commandPlan(answer.records[0]?.record)?.pose_limited;
    return limited
      ? { kind: "closest_pose", advice: limited.advice }
      : { kind: "accepted" };
  } catch (error: unknown) {
    return { kind: "rejected", message: errorMessage(error) };
  }
}

export function ManualControl({
  stationState,
  motionLimits,
  armName,
  changingArmPower,
  active,
  target,
  setTarget,
  projection,
  setProjection,
  projectionValidity,
  dragging,
  globalStopHandler,
}: ManualProps) {
  const held = useSyncExternalStore(
    manualLease.onChange,
    manualLease.holdsLease,
  );
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState("");
  const [previewStatus, setPreviewStatus] = useState(
    "Set a goal to preview its reachable arm pose. Previewing never moves the motors.",
  );
  const [previewUnavailable, setPreviewUnavailable] = useState(false);
  const [confirmed, setConfirmed] = useState<{
    arm: string;
    requestPose: PreviewPose;
    pose: ProposedPose;
  } | null>(null);
  const [jawRanges, setJawRanges] = useState<
    Parameters<typeof unreachableYawLines>[0]
  >([]);
  const [chosenOrientation, setChosenOrientation] =
    useState<Orientation | null>(null);
  const [orientation, setOrientation] = useState<Orientation | null>(null);
  const [approach, setApproach] = useState("solver");
  const [opening, setOpening] = useState(0);
  const [positionDraft, setPositionDraft] = useState<[string, string, string]>([
    "",
    "",
    "",
  ]);
  const [followEnabled, setFollowEnabled] = useState(false);
  const [followStatus, setFollowStatus] = useState("");
  const arm = stationState?.arms.find((candidate) => candidate.arm === armName);
  const armCanBeManuallyControlled = armAcceptsManualControl(arm?.mode ?? null);
  const manualView = manualControlView(
    stationState?.lease ?? null,
    held,
    manualLease.label,
  );
  const blockedReason = changingArmPower
    ? "Wait for the arm power change to finish."
    : !arm
      ? "Waiting for the selected arm."
      : !armCanBeManuallyControlled
        ? "Enable the arm to use manual control."
        : manualControlBlockedReason(manualView);
  const latestArm = useRef<ArmState | undefined>(arm);
  const latestTarget = useRef<Position | null>(target);
  const latestOrientation = useRef<Orientation | null>(orientation);
  const latestProjection = useRef(projection);
  const latestOpening = useRef(opening);
  const requestedLimits = arm?.controller === "ik" ? motionLimits : undefined;
  const latestMotionLimits = useRef(requestedLimits);
  const follow = useRef<Follow | null>(null);
  const gripper = useRef<Gripper | null>(null);
  const projectionActor = useRef<ProjectionController | null>(null);
  const commandInFlight = useRef(false);
  // INVARIANT: Bumped when motion is cancelled; a move started under an older value is stale.
  const motionEpoch = useRef(0);
  // INVARIANT: Bumped when the orientation changes; a late orientation read is stale.
  const orientationEpoch = useRef(0);
  const pendingGripper = useRef<number | null>(null);
  const lastValidOpening = useRef(0);
  const initializedGripperArm = useRef<string | null>(null);
  const yawTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  latestArm.current = arm;
  latestTarget.current = target;
  latestOrientation.current = orientation;
  latestProjection.current = projection;
  latestOpening.current = opening;
  latestMotionLimits.current = requestedLimits;

  useEffect(() => {
    const liveFollow = createLivePoseFollow(
      () => latestArm.current,
      setFollowStatus,
      () => latestMotionLimits.current?.max_joint_speed,
      () => setFollowEnabled(false),
    );
    const liveGripper = createLiveGripper(() => latestArm.current, setMessage);
    follow.current = liveFollow;
    gripper.current = liveGripper;
    const actor = createProjectionController({
      currentRequest: () => {
        const currentArm = latestArm.current;
        const position = latestTarget.current;
        return currentArm && position
          ? {
              arm: currentArm.arm,
              position,
              pose: previewRequest(
                position,
                latestOrientation.current,
                latestMotionLimits.current,
              ),
            }
          : null;
      },
      preview: (armToPreview, pose) =>
        stationConnection.preview(armToPreview, pose),
      onPending: () => setConfirmed(null),
      onCalculating: () => {
        setPreviewUnavailable(false);
        setPreviewStatus("Calculating preview…");
      },
      onClear: () => {
        setConfirmed(null);
        setChosenOrientation(null);
        setJawRanges([]);
        liveFollow.setGoal(null);
        setProjection(null);
        setPreviewUnavailable(false);
        setPreviewStatus("Set a goal to preview its reachable arm pose.");
      },
      onFailure: (detail) => {
        setPreviewUnavailable(true);
        setConfirmed(null);
        liveFollow.setGoal(null);
        setProjection(
          latestProjection.current
            ? { ...latestProjection.current, stale: true }
            : null,
        );
        setPreviewStatus(
          `Preview unavailable: ${detail}. Any visible ghost is stale and cannot be moved to.`,
        );
      },
      onSolution: (solved, confirm) => {
        const currentArm = latestArm.current;
        if (!currentArm || currentArm.arm !== solved.arm) return;
        const result = interpretProjection(solved, confirm);
        setJawRanges(result.jawYawRanges);
        setChosenOrientation(result.chosenOrientation);
        if (result.kind === "unavailable") {
          setPreviewUnavailable(true);
          setConfirmed(null);
          liveFollow.setGoal(null);
          setProjection(
            latestProjection.current
              ? { ...latestProjection.current, stale: true }
              : null,
          );
          setPreviewStatus(
            `Preview unavailable: ${result.reason}. Any visible ghost is stale and cannot be moved to.`,
          );
          return;
        }
        setPreviewUnavailable(false);
        setProjection({
          arm: currentArm.arm,
          joints: result.joints,
          opening: latestOpening.current,
          withinTolerance: result.withinTolerance,
          stale: false,
        });
        setConfirmed(
          result.confirmedPose
            ? {
                arm: solved.arm,
                requestPose: solved.requestPose,
                pose: result.confirmedPose,
              }
            : null,
        );
        setPreviewStatus(result.message);
      },
    });
    projectionActor.current = actor;
    return () => {
      actor.destroy();
      if (yawTimer.current !== null) clearTimeout(yawTimer.current);
      liveFollow.destroy();
      liveGripper.destroy();
      follow.current = null;
      gripper.current = null;
      projectionActor.current = null;
    };
  }, [setProjection]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: Orientation and arm changes require a new projection.
  useEffect(() => {
    setConfirmed(null);
    if (target) projectionActor.current?.targetChanged();
    else projectionActor.current?.clear();
  }, [target, orientation, armName, requestedLimits]);

  useEffect(() => {
    setPositionDraft(
      target
        ? [
            metersToMillimeters(target[0]).toFixed(1),
            metersToMillimeters(target[1]).toFixed(1),
            metersToMillimeters(target[2]).toFixed(1),
          ]
        : ["", "", ""],
    );
  }, [target]);

  useEffect(() => {
    follow.current?.setDragging(dragging);
  }, [dragging]);

  useEffect(() => {
    if (projection?.stale) {
      follow.current?.setGoal(null);
      pendingGripper.current = null;
      return;
    }
    if (projectionValidity === true && projection) {
      lastValidOpening.current = projection.opening;
      follow.current?.setGoal(projection.joints);
      if (followEnabled && pendingGripper.current === projection.opening) {
        gripper.current?.setOpening(projection.opening);
        pendingGripper.current = null;
      }
    } else if (projectionValidity === false) {
      follow.current?.setGoal(null);
      if (pendingGripper.current !== null && projection) {
        pendingGripper.current = null;
        setOpening(lastValidOpening.current);
        setProjection({ ...projection, opening: lastValidOpening.current });
        setMessage(
          "Gripper preview blocked: the jaw would go below the table surface.",
        );
      }
    }
  }, [followEnabled, projectionValidity, projection, setProjection]);

  useEffect(() => {
    if (
      arm?.gripper_opening !== null &&
      arm?.gripper_opening !== undefined &&
      initializedGripperArm.current !== arm.arm
    ) {
      initializedGripperArm.current = arm.arm;
      const measuredOpening = MathUtils.clamp(arm.gripper_opening, 0, 1);
      lastValidOpening.current = measuredOpening;
      setOpening(measuredOpening);
    }
  }, [arm]);

  useEffect(() => {
    if (!followEnabled) return;
    if (
      blockedReason !== null ||
      !arm ||
      arm.controller === "policy" ||
      !["holding", "moving"].includes(arm.mode)
    ) {
      setFollowEnabled(false);
      follow.current?.disarm(
        `Follow stopped: ${blockedReason ?? `arm is ${arm?.mode ?? "unavailable"}`}`,
      );
      gripper.current?.hold();
    }
  }, [arm, blockedReason, followEnabled]);

  const stopFollowing = useCallback((): void => {
    motionEpoch.current += 1;
    pendingGripper.current = null;
    setFollowEnabled(false);
    follow.current?.setEnabled(false);
    gripper.current?.hold();
  }, []);

  useEffect(() => {
    globalStopHandler.current = stopFollowing;
    return () => {
      if (globalStopHandler.current === stopFollowing)
        globalStopHandler.current = null;
    };
  }, [globalStopHandler, stopFollowing]);

  const submit = useCallback(
    async (armToCommand: string, command: StationCommand): Promise<boolean> => {
      if (command.kind === "stop" || command.kind === "disable") {
        const result = await requestManualCommand(armToCommand, command);
        setMessage(commandMessage(result));
        return result.kind !== "rejected";
      }
      // INVARIANT: A second command while one is out is refused, not queued.
      if (commandInFlight.current) {
        setMessage("Waiting for the previous command to be submitted.");
        return false;
      }
      commandInFlight.current = true;
      setPending(true);
      try {
        const result = await requestManualCommand(armToCommand, command);
        setMessage(commandMessage(result));
        return result.kind !== "rejected";
      } finally {
        commandInFlight.current = false;
        setPending(false);
      }
    },
    [],
  );

  function setCurrentTarget(): void {
    if (!arm) return;
    syncCurrentPosePreview(arm, setOpening, (position) =>
      setTarget([position[0] ?? 0, position[1] ?? 0, position[2] ?? 0]),
    );
  }

  const canStartTarget = active && blockedReason === null && arm?.tool != null;
  const initializeTarget = useEffectEvent(() => {
    if (latestTarget.current === null) setCurrentTarget();
  });
  useEffect(() => {
    if (canStartTarget) initializeTarget();
  }, [canStartTarget]);

  function changeApproach(value: string): void {
    orientationEpoch.current += 1;
    setApproach(value);
    follow.current?.stop();
    if (value === "solver") setOrientation(null);
    else if (value === "current" && arm?.tool) {
      const loadEpoch = orientationEpoch.current;
      readCurrentOrientation(arm.arm, arm.tool.position, arm.tool.quaternion)
        .then((loaded) => {
          if (loadEpoch !== orientationEpoch.current) return;
          if (loaded) setOrientation(loaded);
          else {
            setOrientation(null);
            setApproach("solver");
            setMessage(
              "The station could not read the arm's current orientation.",
            );
          }
        })
        .catch((error: unknown) => setMessage(errorMessage(error)));
    } else if (value === "down" || value === "forward")
      setOrientation({
        kind: "grasp",
        approach: value,
        jaw_yaw_radians:
          (orientation ?? chosenOrientation)?.jaw_yaw_radians ?? 0,
      });
  }

  function move(): void {
    if (
      !arm ||
      !confirmed ||
      manualMoveBlockedReason({
        ownershipReason: blockedReason,
        armMode: arm.mode,
        hasTarget: target !== null,
        hasConfirmedPreview: previewMatchesSelection({
          confirmed,
          arm: arm.arm,
          projectionArm: projection?.arm ?? null,
          target,
          orientation,
          motionLimits: requestedLimits,
        }),
        projectionValidity,
      }) !== null
    )
      return;
    const command = moveCommandForPreview(
      arm.controller,
      confirmed.pose,
      confirmed.requestPose.max_joint_speed,
      confirmed.requestPose,
    );
    if (!command) {
      setMessage(
        "Command not sent: the station gave no orientation for this goal.",
      );
      return;
    }
    const epoch = motionEpoch.current;
    void movePreviewedGripper(
      () => submit(arm.arm, command),
      () => epoch === motionEpoch.current && latestArm.current?.arm === arm.arm,
      arm.gripper_opening !== null,
      opening,
      (value) => gripper.current?.setOpening(value),
    );
  }

  const displayedOrientation = orientation ?? chosenOrientation;
  const displayedYawDegrees = Math.round(
    angularDegrees(displayedOrientation?.jaw_yaw_radians ?? 0, "rad"),
  );
  const confirmedForSelection = previewMatchesSelection({
    confirmed,
    arm: armName,
    projectionArm: projection?.arm ?? null,
    target,
    orientation,
    motionLimits: requestedLimits,
  });
  const moveBlocked = manualMoveBlockedReason({
    ownershipReason: blockedReason,
    armMode: arm?.mode ?? null,
    hasTarget: target !== null,
    hasConfirmedPreview: confirmedForSelection,
    projectionValidity,
  });
  return (
    <section className="manual-control" aria-label="Manual control">
      {!held && !armCanBeManuallyControlled && !changingArmPower && (
        <p>
          {arm?.mode === "disabled" || arm?.mode === "reconnected" ? (
            <>
              <a href="#enable-arm">Enable the arm</a> to use manual control.
            </>
          ) : (
            "Connect the arm to use manual control."
          )}
        </p>
      )}
      <fieldset
        className="grid gap-3 border-0 p-0"
        disabled={!held || !armCanBeManuallyControlled || changingArmPower}
      >
        <legend className="sr-only">Manual controls</legend>
        <h3>Goal</h3>
        <div className="control-actions">
          <Button
            variant="outline"
            disabled={!arm?.tool || blockedReason !== null}
            onClick={setCurrentTarget}
          >
            Use current position
          </Button>
          {target && (
            <Button
              variant="outline"
              onClick={() => {
                stopFollowing();
                setTarget(null);
                setProjection(null);
              }}
            >
              Clear goal
            </Button>
          )}
        </div>
        {arm?.gripper_opening != null && (
          <label>
            Gripper {Math.round(opening * 100)}% open
            <input
              type="range"
              min="0"
              max="100"
              step="1"
              value={Math.round(opening * 100)}
              disabled={blockedReason !== null}
              onChange={(event) => {
                const next = Number(event.currentTarget.value) / 100;
                setOpening(next);
                if (projection) {
                  pendingGripper.current = followEnabled ? next : null;
                  setProjection({ ...projection, opening: next });
                } else if (followEnabled) {
                  gripper.current?.setOpening(next);
                }
              }}
            />
          </label>
        )}
        <details className="control-options">
          <summary>Motion options</summary>
          <form
            className="target-coordinates"
            aria-label="Goal coordinates"
            onSubmit={(event) => {
              event.preventDefault();
              if (blockedReason !== null) return;
              if (positionDraft.some((value) => value.trim() === "")) {
                setMessage("Enter all three goal coordinates.");
                return;
              }
              const [x, y, z] = positionDraft;
              const coordinates: Position = [
                millimetersToMeters(Number(x)),
                millimetersToMeters(Number(y)),
                millimetersToMeters(Number(z)),
              ];
              if (!coordinates.every(Number.isFinite)) {
                setMessage("Goal coordinates must be finite numbers.");
                return;
              }
              setTarget(coordinates);
            }}
          >
            <div className="coordinate-fields">
              {(["X", "Y", "Z"] as const).map((axis, index) => (
                <label key={axis} htmlFor={`target-coordinate-${axis}`}>
                  {axis} (mm)
                  <Input
                    id={`target-coordinate-${axis}`}
                    type="number"
                    step="any"
                    value={positionDraft[index]}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setPositionDraft((previous) => {
                        const next: [string, string, string] = [...previous];
                        next[index] = value;
                        return next;
                      });
                    }}
                  />
                </label>
              ))}
            </div>
            <Button
              type="submit"
              variant="outline"
              className="w-fit"
              disabled={blockedReason !== null}
            >
              Preview coordinates
            </Button>
          </form>
          <label htmlFor="manual-approach">
            Approach
            <NativeSelect
              id="manual-approach"
              value={approach}
              onChange={(event) =>
                void changeApproach(event.currentTarget.value)
              }
            >
              <NativeSelectOption value="solver">
                Solver chooses
              </NativeSelectOption>
              <NativeSelectOption value="down">Down</NativeSelectOption>
              <NativeSelectOption value="forward">Forward</NativeSelectOption>
              <NativeSelectOption value="current">
                Current pose
              </NativeSelectOption>
            </NativeSelect>
          </label>
          {displayedOrientation && (
            <label>
              Jaw yaw {displayedYawDegrees}°
              <input
                type="range"
                min="0"
                max="360"
                step="1"
                value={displayedYawDegrees}
                onChange={(event) => {
                  orientationEpoch.current += 1;
                  follow.current?.setDragging(true);
                  if (yawTimer.current !== null) clearTimeout(yawTimer.current);
                  yawTimer.current = setTimeout(
                    () => follow.current?.setDragging(false),
                    1000,
                  );
                  setOrientation({
                    kind: "grasp",
                    approach: displayedOrientation.approach,
                    jaw_yaw_radians: angularRadians(
                      Number(event.currentTarget.value) % 360,
                      "deg",
                    ),
                  });
                }}
              />
            </label>
          )}
          {displayedOrientation && (
            <p>
              {describeApproach(displayedOrientation.approach)} ·{" "}
              {unreachableYawLines(jawRanges).join(" · ")}
            </p>
          )}
        </details>
        <div className="control-actions">
          <Button disabled={moveBlocked !== null || pending} onClick={move}>
            {pending ? "Sending move…" : "Move to goal"}
          </Button>
        </div>
        <p className="manual-target-status">
          {formatMotionDetail(
            previewUnavailable
              ? previewStatus
              : dragging
                ? "Release the goal to check the preview."
                : target
                  ? projectionValidity === false ||
                    arm?.mode !== "holding" ||
                    (confirmedForSelection && projectionValidity === null)
                    ? (moveBlocked ?? previewStatus)
                    : previewStatus
                  : "Drag a goal in the robot view or use the current pose.",
          )}
        </p>
        <label className="flex items-center gap-2" htmlFor="follow-target">
          <Switch
            id="follow-target"
            checked={followEnabled}
            disabled={
              !arm ||
              arm.controller === "policy" ||
              blockedReason !== null ||
              !["holding", "moving"].includes(arm.mode)
            }
            onCheckedChange={(enabled) => {
              setFollowEnabled(enabled);
              gripper.current?.hold();
              follow.current?.setEnabled(enabled);
            }}
          />
          Follow goal while dragging
        </label>
      </fieldset>
      <div className="empty:hidden" role="status">
        {message && <p>{formatMotionDetail(message)}</p>}
        {followStatus && <p>{formatMotionDetail(followStatus)}</p>}
      </div>
    </section>
  );
}
