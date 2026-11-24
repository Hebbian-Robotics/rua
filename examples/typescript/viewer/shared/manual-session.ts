import { apiSchemas } from "hebbian-rua";
import { z } from "zod";

const leaseId = z.uuid();
const operationId = z.uuid();
const arm = z.string();
const goal = z.object({
  sequence: z.int().nonnegative(),
  joints: z.array(z.number()),
  gripper_opening: z.number().optional(),
  expected_revision: z.int().nonnegative().optional(),
});

export const manualRequest = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("heartbeat") }),
  z.object({ kind: z.literal("acquire"), label: z.string() }),
  z.object({ kind: z.literal("return"), leaseId }),
  z.object({ kind: z.literal("cancel"), leaseId, operationId }),
  z.object({
    kind: z.literal("send"),
    leaseId,
    arm,
    operationId,
    command: apiSchemas.shape.StationCommand,
  }),
  z.object({ kind: z.literal("rehome"), leaseId, arm, operationId }),
  z.object({ kind: z.literal("goal"), leaseId, operationId, goal }),
  z.object({
    kind: z.literal("power"),
    arm,
    waitForOutcome: z.boolean().optional(),
    action: z.enum([
      "enable",
      "disable",
      "clearFault",
      "approveRecovery",
      "stop",
    ]),
  }),
  z.object({
    kind: z.literal("preview"),
    arm,
    pose: apiSchemas.shape.PreviewPose,
  }),
]);
export type ManualRequest = z.input<typeof manualRequest>;

export const viewerEvent = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("session"), id: z.uuid() }),
  z.object({ kind: z.literal("heartbeat") }),
  z.object({ kind: z.literal("connected"), connected: z.boolean() }),
  z.object({ kind: z.literal("leaseLost"), leaseId }),
  z.object({ kind: z.literal("state"), state: apiSchemas.shape.StationState }),
  z.object({ kind: z.literal("run"), run: apiSchemas.shape.LiveRun }),
  z.object({
    kind: z.literal("sequence"),
    sequence: apiSchemas.shape.ActionSequenceRecord.nullable(),
  }),
]);
export type ViewerEvent = z.output<typeof viewerEvent>;
export const manualGrant = z.object({ leaseId, leaseSeconds: z.number() });
