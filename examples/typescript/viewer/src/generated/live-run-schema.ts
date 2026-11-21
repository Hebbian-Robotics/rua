import * as z from "zod";
export const liveRunSchema = z
  .object({
    revision: z.number().int(),
    spans: z.array(
      z
        .object({
          traceId: z.string().regex(/^[0-9a-f]{32}$/),
          spanId: z.string().regex(/^[0-9a-f]{16}$/),
          parentSpanId: z
            .union([z.string().regex(/^[0-9a-f]{16}$/), z.null()])
            .default(null),
          name: z.string(),
          startTimeUnixNano: z.string(),
          endTimeUnixNano: z.string(),
          attributes: z.record(z.string(), z.any()),
          status: z
            .object({
              code: z.enum([
                "STATUS_CODE_UNSET",
                "STATUS_CODE_OK",
                "STATUS_CODE_ERROR",
              ]),
              message: z.union([z.string(), z.null()]).default(null),
            })
            .strict(),
        })
        .strict(),
    ),
    usage: z.union([
      z
        .object({
          runId: z.string(),
          turns: z.number().int(),
          inputTokens: z.number().int(),
          cachedInputTokens: z.number().int(),
          outputTokens: z.number().int(),
          reasoningTokens: z.number().int(),
        })
        .strict(),
      z.null(),
    ]),
    memory: z
      .object({
        task: z.union([z.string(), z.null()]).default(null),
        plan: z
          .union([
            z.array(
              z
                .object({
                  text: z.string(),
                  status: z.enum(["pending", "active", "done", "failed"]),
                })
                .strict(),
            ),
            z.null(),
          ])
          .default(null),
        notes: z.array(z.string()),
      })
      .strict(),
  })
  .strict();
export type LiveRun = z.infer<typeof liveRunSchema>;
