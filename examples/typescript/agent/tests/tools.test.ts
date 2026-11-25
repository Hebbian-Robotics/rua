import { expect, test } from "vitest";
import {
  AGENT_TOOLS,
  TaskMemory,
  updatePlanInputSchema,
} from "../src/tools.js";

test("a plan may have at most one active step", () => {
  const plan = (statuses: string[]) => ({
    steps: statuses.map((status, index) => ({ text: `step ${index}`, status })),
  });
  expect(
    updatePlanInputSchema.safeParse(plan(["done", "active", "pending"]))
      .success,
  ).toBe(true);
  expect(
    updatePlanInputSchema.safeParse(plan(["active", "active"])).success,
  ).toBe(false);
});

test("task memory shows the plan and notes on one line each, plan first", () => {
  const memory = new TaskMemory("Touch the block");
  expect(memory.observationLines()).toEqual([]);
  memory.plan = updatePlanInputSchema.parse({
    steps: [
      { text: "look\nclosely", status: "done" },
      { text: "reach", status: "active" },
    ],
    note: "n",
  });
  memory.notes = ["block is red\r\nand small"];
  expect(memory.observationLines()).toEqual([
    "Current plan:",
    "1. [done] look closely",
    "2. [active] reach",
    "Notes:",
    "- block is red and small",
  ]);
  expect(JSON.parse(memory.viewerSnapshot())).toEqual({
    task: "Touch the block",
    plan: memory.plan.steps,
    notes: ["block is red\r\nand small"],
  });
});

test("the loop's tools describe themselves with plain JSON schemas", () => {
  for (const tool of AGENT_TOOLS) {
    expect(tool.parametersSchema["type"]).toBe("object");
    expect(tool.parametersSchema).not.toHaveProperty("$schema");
  }
});
