// The run API of the agent server, shared by the server and the viewer.
// Times are Unix milliseconds.

import { z } from "zod";
import {
  INFERENCE_MODES,
  PROVIDERS,
  REASONING_EFFORTS,
} from "../../agent/src/providers/types.js";

export const RUN_STREAM_HEARTBEAT_EVENT = "heartbeat";
export const MAX_PROMPT_IMAGE_BASE64_LENGTH =
  4 * Math.ceil((2 * 1024 * 1024) / 3);

const attachmentNameSchema = z.string().trim().min(1).max(255);
const promptImageSchema = z.object({
  filename: attachmentNameSchema,
  jpegBase64: z.base64().max(MAX_PROMPT_IMAGE_BASE64_LENGTH).startsWith("/9j/"),
});
export type PromptImage = z.infer<typeof promptImageSchema>;

export const reasoningEffortSchema = z.enum(REASONING_EFFORTS);
export const inferenceModeSchema = z.enum(INFERENCE_MODES);
export const providerSchema = z.enum(PROVIDERS);
export type Provider = z.infer<typeof providerSchema>;

export const motionLimitsSchema = z.object({
  max_joint_speed: z.number().positive().optional(),
  max_joint_acceleration: z.number().positive().optional(),
  max_joint_jerk: z.number().positive().optional(),
});
export type MotionLimits = z.output<typeof motionLimitsSchema>;

export const runRequestSchema = z.object({
  motionLimits: motionLimitsSchema.optional(),
  arm: z.string().min(1),
  task: z.string().min(1),
  cameras: z.array(z.string().min(1)).default([]),
  optionalCameras: z.array(z.string().min(1)).default([]),
  promptImage: promptImageSchema.optional(),
  settings: z.object({
    provider: providerSchema,
    model: z.string().trim().min(1),
    reasoningEffort: reasoningEffortSchema.nullable().default(null),
    // Runs saved before inference modes read as standard.
    inferenceMode: inferenceModeSchema.nullable().default(null),
  }),
});
export type RunRequest = z.input<typeof runRequestSchema>;
export type ParsedRunRequest = z.output<typeof runRequestSchema>;

const storedRunRequestSchema = runRequestSchema
  .omit({ promptImage: true })
  .extend({
    attachmentFilename: attachmentNameSchema.optional(),
    settings: runRequestSchema.shape.settings.extend({
      provider: providerSchema.nullable().default(null),
      model: z.string().min(1).nullable().default(null),
    }),
  });

const runOutcomeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("completed") }),
  z.object({ kind: z.literal("incomplete"), detail: z.string() }),
  z.object({ kind: z.literal("stopped") }),
  z.object({ kind: z.literal("interrupted") }),
  z.object({
    kind: z.literal("failed"),
    detail: z.string(),
  }),
]);

const runPhaseSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("running"), startedAt: z.number() }),
  z.object({ state: z.literal("stopping"), startedAt: z.number() }),
  z.object({
    state: z.literal("ended"),
    startedAt: z.number().nullable(),
    endedAt: z.number(),
    outcome: runOutcomeSchema,
    response: z.string().optional(),
  }),
]);

export const runSchema = z.object({
  id: z.string(),
  createdAt: z.number(),
  request: storedRunRequestSchema,
  phase: runPhaseSchema,
  output: z.array(z.string()),
  persistenceError: z.string().optional(),
  usage: z
    .object({
      run_id: z.string(),
      turns: z.number(),
      input_tokens: z.number(),
      cached_input_tokens: z.number(),
      output_tokens: z.number(),
      reasoning_tokens: z.number(),
    })
    .optional(),
});
export type Run = z.infer<typeof runSchema>;
export type RunPhase = z.infer<typeof runPhaseSchema>;

export const currentRunSchema = runSchema.nullable();
export const modelProvidersSchema = z.array(
  z.object({
    provider: providerSchema,
    label: z.string(),
    available: z.boolean(),
    models: z.array(z.object({ id: z.string(), label: z.string() })),
  }),
);
export const healthSchema = z.object({
  ok: z.literal(true),
  manualOnly: z.boolean(),
});

export const runEventSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("snapshot"), run: runSchema }),
  z.object({ type: z.literal("phase"), phase: runPhaseSchema }),
  z.object({ type: z.literal("output"), line: z.string() }),
  z.object({
    type: z.literal("persistence_error"),
    detail: z.string().nullable(),
  }),
]);
export type RunEvent = z.infer<typeof runEventSchema>;

export const errorBodySchema = z.object({ detail: z.string() });

export function isRunActive(run: Run | null): boolean {
  return (
    run !== null &&
    (run.phase.state === "running" || run.phase.state === "stopping")
  );
}
