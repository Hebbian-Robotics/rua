import * as z from "zod";
export const mediaStatusSchema = z.object({
  cameras: z
    .record(
      z.string(),
      z.union([
        z.union([
          z.object({ state: z.literal("resolving") }),
          z.object({ state: z.literal("starting") }),
          z.object({
            publication: z.union([
              z.object({ state: z.literal("disabled") }),
              z.object({ state: z.literal("connecting") }),
              z.object({ state: z.literal("publishing") }),
              z.object({
                detail: z.union([z.string(), z.null()]).optional(),
                state: z.literal("reconnecting"),
              }),
            ]),
            state: z.literal("capturing"),
          }),
          z.object({ state: z.literal("disconnected") }),
          z.object({ state: z.literal("ambiguous") }),
          z.object({ state: z.literal("stalled") }),
          z.object({ detail: z.string(), state: z.literal("failed") }),
        ]),
        z.never(),
      ]),
    )
    .superRefine((value, ctx) => {
      for (const key in value) {
        let evaluated = false;
        if (key.match(/^[a-z0-9_]{1,40}$/)) {
          evaluated = true;
          const result = z
            .union([
              z.object({ state: z.literal("resolving") }),
              z.object({ state: z.literal("starting") }),
              z.object({
                publication: z.union([
                  z.object({ state: z.literal("disabled") }),
                  z.object({ state: z.literal("connecting") }),
                  z.object({ state: z.literal("publishing") }),
                  z.object({
                    detail: z.union([z.string(), z.null()]).optional(),
                    state: z.literal("reconnecting"),
                  }),
                ]),
                state: z.literal("capturing"),
              }),
              z.object({ state: z.literal("disconnected") }),
              z.object({ state: z.literal("ambiguous") }),
              z.object({ state: z.literal("stalled") }),
              z.object({ detail: z.string(), state: z.literal("failed") }),
            ])
            .safeParse(value[key]);
          if (!result.success) {
            ctx.addIssue({
              path: [key],
              code: "custom",
              message: `Invalid input: Key matching regex /${key}/ must match schema`,
              params: {
                issues: result.error.issues,
              },
            });
          }
        }
        if (!evaluated) {
          const result = z.never().safeParse(value[key]);
          if (!result.success) {
            ctx.addIssue({
              path: [key],
              code: "custom",
              message: `Invalid input: must match catchall schema`,
              params: {
                issues: result.error.issues,
              },
            });
          }
        }
      }
    }),
  service: z.union([
    z.object({ state: z.literal("running") }),
    z.object({ last_exit: z.string(), state: z.literal("restarting") }),
    z.object({ reason: z.string(), state: z.literal("unavailable") }),
  ]),
});
export type MediaStatus = z.infer<typeof mediaStatusSchema>;
