import { z } from "zod";
import type { ToolSpec } from "./providers/types.js";

export const FINISH_TOOL = "finish";
const UPDATE_PLAN_TOOL = "update_plan";
export const UPDATE_NOTES_TOOL = "update_notes";
export const MEMORY_TOOLS: readonly string[] = [
  UPDATE_PLAN_TOOL,
  UPDATE_NOTES_TOOL,
];

const planStepSchema = z.object({
  text: z.string(),
  status: z.enum(["pending", "active", "done", "failed"]),
});
type PlanStep = z.infer<typeof planStepSchema>;

export const finishInputSchema = z.object({ reason: z.string() }).strict();

export const updatePlanInputSchema = z
  .object({
    steps: z.array(planStepSchema.strict()),
    note: z.string().nullish(),
  })
  .strict()
  .refine(
    (plan) => plan.steps.filter((step) => step.status === "active").length <= 1,
    {
      message: "A plan may have at most one active step",
    },
  );
export type UpdatePlanInput = z.infer<typeof updatePlanInputSchema>;

export const updateNotesInputSchema = z
  .object({ notes: z.array(z.string()) })
  .strict();

interface MemorySnapshot {
  task: string | null;
  plan: PlanStep[] | null;
  notes: string[];
}

function singleLine(text: string): string {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: the separators Python's splitlines uses
  return text.split(/\r\n|[\n\r\v\f\x1c-\x1e\x85\u2028\u2029]/).join(" ");
}

export class TaskMemory {
  plan: UpdatePlanInput | null = null;
  notes: string[] = [];

  constructor(readonly task: string | null = null) {}

  viewerSnapshot(): string {
    const snapshot: MemorySnapshot = {
      task: this.task,
      plan: this.plan?.steps ?? null,
      notes: this.notes,
    };
    return JSON.stringify(snapshot);
  }

  observationLines(): string[] {
    const planLines = this.plan
      ? [
          "Current plan:",
          ...this.plan.steps.map(
            (step, index) =>
              `${index + 1}. [${step.status}] ${singleLine(step.text)}`,
          ),
        ]
      : [];
    const noteLines =
      this.notes.length > 0
        ? ["Notes:", ...this.notes.map((note) => `- ${singleLine(note)}`)]
        : [];
    return [...planLines, ...noteLines];
  }
}

function jsonSchema(schema: z.ZodType): Record<string, unknown> {
  const { $schema: _dialect, ...rest } = z.toJSONSchema(schema, {
    io: "input",
  });
  return rest;
}

export const AGENT_TOOLS: ToolSpec[] = [
  {
    name: UPDATE_PLAN_TOOL,
    description:
      "Replace the current task plan. Each step has text and status (pending, active, done, failed). The current plan is shown at the top of every observation.",
    parametersSchema: jsonSchema(updatePlanInputSchema),
  },
  {
    name: UPDATE_NOTES_TOOL,
    description:
      "Replace this run's notes. Pass the complete list of notes to keep. Notes are shown with the plan at the top of every observation.",
    parametersSchema: jsonSchema(updateNotesInputSchema),
  },
  {
    name: FINISH_TOOL,
    description: "End the run.",
    parametersSchema: jsonSchema(finishInputSchema),
  },
];

const AGENT_INSTRUCTIONS = `update_plan stores a task plan and may accompany motion tools in the same turn.
Each update replaces the whole plan. update_notes stores run-local notes and replaces
the whole notes list. Call finish, alone in its turn, to report the task outcome.
Operate the configured SO-101 using Cartesian targets and gripper actions.
Pose targets are in the station world frame. In the standard SO-101 layout,
+X is forward, +Y is left when facing +X, and +Z is up.
Camera image directions depend on the camera pose.
Choose the next useful action for the task. Preview answers a specific feasibility
question without moving the arm. The loop supplies arm state and available camera
images after action batches when the run continues. Observe can supply an additional
view or wait for a scene change.
For a limited move, pose_limited.advice, when present, describes the shortfall,
where the arm went, and a reachable alternative. A stopped_short move settled
outside tolerance. Its completion detail describes the remaining error, which
does not identify the cause on its own. Use the outcome and observations to
decide whether to revise the target or finish.`;

export function systemPrompt(robotContext: string): string {
  return `Use the robot tools to carry out the task. You are operating in a supervised development workspace. Robot context: ${robotContext}\n\n${AGENT_INSTRUCTIONS}`;
}
