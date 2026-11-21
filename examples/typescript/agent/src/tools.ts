// The loop's own tools, beside the robot's: finishing, and the run's plan and notes.

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
Operate the single configured SO-101 arm using Cartesian targets and gripper actions.
In the base frame, +X is forward from the arm, +Y is left, and +Z is up.
Camera image directions depend on the camera pose.
Choose the next useful action; compare alternatives only when they change it.
Use preview to answer a specific feasibility question, not to rediscover documented
frames, joint signs or guard semantics. Establish the current workspace from images.
Fresh state and camera images follow each action batch automatically; observe is for
an additional view or a deliberate wait, not a routine observe(0) after motion.
After a limited or incomplete action, read its summary before replanning: a limited
target's pose_limited.advice gives the shortfall, where the arm went and an alternative
to try; a stopped_short move's completion detail gives how far short it stopped and
where the tool rests, usually against the table or an object (compare with surface_guard).
Check the images, then aim the next target where it can be reached; do not repeat it.`;

export function systemPrompt(robotContext: string): string {
  return `Use the robot tools to carry out the task. You are operating in a supervised development workspace where manipulation mistakes are fine. Robot context: ${robotContext}\n\n${AGENT_INSTRUCTIONS}`;
}
