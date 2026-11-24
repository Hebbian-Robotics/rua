import * as z from "zod";
export const recordingStatusSchema = z.union([
  z.object({ state: z.literal("idle") }),
  z.object({ name: z.string(), state: z.literal("recording") }),
  z.object({ name: z.string(), state: z.literal("stopping") }),
  z.object({
    name: z.string(),
    state: z.literal("finished"),
    warning: z
      .union([z.literal("controller_interrupted"), z.null()])
      .optional(),
  }),
  z.object({
    detail: z.string(),
    name: z.string(),
    state: z.literal("failed"),
  }),
]);
export type RecordingStatus = z.infer<typeof recordingStatusSchema>;
