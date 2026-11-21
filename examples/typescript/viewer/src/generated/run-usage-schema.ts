import * as z from "zod";
export const runUsageSchema = z
  .object({
    runId: z.string(),
    turns: z.number().int(),
    inputTokens: z.number().int(),
    cachedInputTokens: z.number().int(),
    outputTokens: z.number().int(),
    reasoningTokens: z.number().int(),
  })
  .strict();
export type RunUsage = z.infer<typeof runUsageSchema>;
