// Builders for station events and model-facing lease test doubles.

import { vi } from "vitest";
import type { ApiSchemas, CommandRecord, Lease } from "../src/index.js";
export function commandRecord(
  commandId: string,
  finishedAt: number | null = null,
  outcome: ApiSchemas["CommandOutcome"] = "completed",
): CommandRecord {
  return {
    command_id: commandId,
    label: "enable",
    accepted_unix_nano: "1000000000",
    command: { kind: "control", action: "enable" },
    phase:
      finishedAt === null
        ? { kind: "pending" }
        : outcome === "interrupted"
          ? {
              kind: "outcome_unknown",
              finished_unix_nano: String(finishedAt * 1e9),
              detail: null,
            }
          : {
              kind: "finished",
              achieved: null,
              completion: {
                outcome,
                finished_unix_nano: String(finishedAt * 1e9),
                detail: null,
                gripper: null,
              },
            },
    checkpoints: [],
    trace_context: null,
  };
}

export function recordFrame(
  arm: string,
  seq: number,
  record: ReturnType<typeof commandRecord>,
) {
  return {
    kind: "frame",
    arm,
    frame: {
      seq,
      emitted_unix_nano: "1",
      event: { ...record, kind: "record" },
    },
  };
}

interface ArmOptions {
  arm?: string;
  mode?: string;
  sampledAt?: number | null;
  activeCommandId?: string;
  lastCommandId?: string;
  revision?: number;
}

function armState(options: ArmOptions) {
  return {
    active_command: options.activeCommandId
      ? commandRecord(options.activeCommandId)
      : null,
    arm: options.arm ?? "follower",
    commanded_joints: null,
    controller: "ik",
    fault: null,
    gripper_opening: null,
    last_command: options.lastCommandId
      ? commandRecord(options.lastCommandId, 2)
      : null,
    measured_joint_efforts: [],
    measured_joint_velocities: [],
    measured_joints: [],
    mode: options.mode ?? "holding",
    recovery_issue: null,
    rest_pose: null,
    sampled_unix_nano:
      options.sampledAt == null ? null : String(options.sampledAt * 1e9),
    segment: null,
    status: "",
    tool: null,
    interlocks: [],
    revision: options.revision ?? 0,
  };
}

export function stateMessage(
  arms: ArmOptions[],
  lease: { holder: string } | null = null,
) {
  return {
    kind: "state",
    state: {
      arms: arms.map(armState),
      lease:
        lease === null
          ? null
          : {
              holder: lease.holder,
              label: "x",
              required_cameras: [],
              on_interruption: "wait_for_approval",
              expires_in_seconds: 15,
            },
      description: null,
      jaw_closing_axis: "x",
      joint_names: [],
      max_joint_speed: 1,
      robot: null,
      surface_guard: null,
    },
  };
}

/** A lease whose commands are vi.fn stubs the test can answer; `isHeld` is true. */
export function fakeLease(overrides: Partial<Lease> = {}): Lease {
  const unexpected = (name: string) =>
    vi.fn(async () => {
      throw new Error(`Unexpected ${name}`);
    });
  return {
    status: {
      holder: "agent",
      label: "test",
      required_cameras: [],
      on_interruption: "wait_for_approval",
      expires_in_seconds: 15,
    },
    leaseSeconds: 15,
    isHeld: () => true,
    arm: vi.fn(),
    sendCommand: unexpected("sendCommand"),
    cancelOperation: unexpected("cancelOperation"),
    updateStreamGoal: unexpected("updateStreamGoal"),
    sendGroup: unexpected("sendGroup"),
    runCommand: unexpected("runCommand"),
    runGroup: unexpected("runGroup"),
    startActionSequence: unexpected("startActionSequence"),
    startCompiledSequence: unexpected("startCompiledSequence"),
    stopArm: vi.fn(async () => undefined),
    reportCamera: vi.fn(async () => undefined),
    stopRecording: vi.fn(async () => undefined),
    return: vi.fn(),
    ...overrides,
  } as Lease;
}
