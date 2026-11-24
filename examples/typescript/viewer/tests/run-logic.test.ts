import { expect, test } from "vitest";
import {
  buildRunRequest,
  chosenLessons,
  decideStart,
  formatElapsed,
  inferenceModeOptions,
  manualLoopCommand,
  type Run,
  type RunFormValues,
  runStatusLine,
  shouldFollowOutput,
} from "../src/run-logic.js";

const startedAt = 1_000_000;

function runIn(phase: Run["phase"]): Run {
  return {
    id: "abc",
    createdAt: startedAt,
    request: {
      arm: "follower",
      task: "stack the cups",
      lessons: [],
      cameras: [],
      optionalCameras: [],
      settings: {
        provider: "openai",
        model: "gpt-6.2-astra",
        reasoningEffort: null,
        inferenceMode: null,
      },
    },
    phase,
    output: [],
  };
}

const running = runIn({ state: "running", startedAt });
const stopping = runIn({ state: "stopping", startedAt });
function ended(
  outcome: Extract<Run["phase"], { state: "ended" }>["outcome"],
  endedAt: number,
) {
  return runIn({ state: "ended", startedAt, endedAt, outcome });
}

const form = {
  task: "  stack the cups  ",
  arm: "follower",
  cameras: "  chest,wrist_r ",
  optionalCameras: "",
  provider: "openai",
  model: "gpt-6.2-astra",
  reasoningEffort: "high",
  inferenceMode: "standard",
} satisfies RunFormValues;
const arms = ["follower", "other"];

test("a described task on a known arm may start, with a trimmed request", () => {
  const decision = decideStart(form, null, arms, false);
  expect(decision.kind).toBe("allowed");
  if (decision.kind !== "allowed") return;
  expect(decision.request).toMatchObject({
    task: "stack the cups",
    cameras: ["chest", "wrist_r"],
    settings: {
      provider: "openai",
      model: "gpt-6.2-astra",
      reasoningEffort: "high",
    },
  });
});

test("run state, a blank task, an unknown arm or a missing model choice blocks Start", () => {
  expect(
    decideStart(
      form,
      ended({ kind: "completed" }, startedAt + 4000),
      arms,
      false,
    ).kind,
  ).toBe("allowed");
  expect(
    decideStart(form, ended({ kind: "stopped" }, startedAt + 4000), arms, false)
      .kind,
  ).toBe("allowed");
  for (const run of [running, stopping]) {
    expect(decideStart(form, run, arms, false)).toMatchObject({
      kind: "blocked",
      reason: expect.stringContaining(run.phase.state),
    });
  }
  expect(decideStart(form, null, arms, true)).toMatchObject({
    kind: "blocked",
    reason: expect.stringContaining("Starting"),
  });
  expect(
    decideStart({ ...form, task: "   " }, null, arms, false),
  ).toMatchObject({ kind: "blocked", reason: expect.stringContaining("task") });
  expect(
    decideStart({ ...form, arm: "ghost" }, null, arms, false),
  ).toMatchObject({ kind: "blocked", reason: "Choose an arm." });
  expect(
    decideStart({ ...form, provider: "", model: "" }, null, arms, false),
  ).toEqual({
    kind: "blocked",
    reason: "Choose a model provider.",
  });
  expect(decideStart({ ...form, model: "" }, null, arms, false)).toEqual({
    kind: "blocked",
    reason: "Choose a model.",
  });
  expect(decideStart(form, null, [], false)).toMatchObject({
    kind: "blocked",
    reason: expect.stringContaining("arms"),
  });
});

test("xhigh starts unless the chosen Gemini model lacks it", () => {
  const xhigh = { ...form, reasoningEffort: "xhigh" } as const;
  expect(decideStart(xhigh, null, arms, false)).toMatchObject({
    kind: "allowed",
    request: { settings: { reasoningEffort: "xhigh" } },
  });
  expect(
    decideStart({ ...xhigh, provider: "openai" }, null, arms, false).kind,
  ).toBe("allowed");
  expect(
    decideStart(
      { ...xhigh, provider: "gemini", model: "gemini-4-hydrogen" },
      null,
      arms,
      false,
    ).kind,
  ).toBe("allowed");
  expect(
    decideStart(
      { ...xhigh, provider: "gemini", model: "gemini-4-carbon" },
      null,
      arms,
      false,
    ),
  ).toMatchObject({
    kind: "blocked",
    reason: expect.stringContaining("gemini-4-carbon"),
  });
});

test("model choices and blank cameras reach the request", () => {
  expect(
    buildRunRequest({
      ...form,
      provider: "openai",
      model: " custom ",
    }).settings,
  ).toEqual({
    provider: "openai",
    model: "custom",
    reasoningEffort: "high",
    inferenceMode: "standard",
  });
  expect(buildRunRequest({ ...form, cameras: "   " }).cameras).toEqual([]);
});

test("each provider offers its own inference modes, Gemini's fast named Priority", () => {
  const labels = (provider?: "openai" | "anthropic" | "gemini") =>
    inferenceModeOptions(provider).map(({ label }) => label);
  expect(labels("openai")).toEqual(["Standard", "Fast", "Ultrafast"]);
  expect(labels("anthropic")).toEqual(["Standard", "Fast"]);
  expect(labels("gemini")).toEqual(["Standard", "Priority"]);
  expect(labels()).toEqual(["Standard"]);
});

test("an inference mode starts only where the provider and model serve it", () => {
  const ultrafast = {
    ...form,
    provider: "openai",
    model: "gpt-6-astra",
    inferenceMode: "ultrafast",
  } as const;
  expect(decideStart(ultrafast, null, arms, false)).toMatchObject({
    kind: "allowed",
    request: { settings: { inferenceMode: "ultrafast" } },
  });
  expect(
    decideStart({ ...ultrafast, model: "gpt-5.6" }, null, arms, false),
  ).toMatchObject({
    kind: "blocked",
    reason: expect.stringContaining("gpt-5.6"),
  });
  expect(
    decideStart(
      { ...ultrafast, provider: "anthropic", model: "claude-fable-5-2" },
      null,
      arms,
      false,
    ),
  ).toMatchObject({
    kind: "blocked",
    reason: expect.stringContaining("not offered by anthropic"),
  });
  const fast = { ...form, inferenceMode: "fast" } as const;
  expect(
    decideStart(
      { ...fast, provider: "anthropic", model: "claude-fable-5-2" },
      null,
      arms,
      false,
    ).kind,
  ).toBe("allowed");
  expect(
    decideStart(
      { ...fast, provider: "anthropic", model: "claude-sonnet-5-5" },
      null,
      arms,
      false,
    ).kind,
  ).toBe("blocked");
  expect(
    decideStart(
      { ...fast, provider: "gemini", model: "gemini-4-argon" },
      null,
      arms,
      false,
    ).kind,
  ).toBe("allowed");
});

test("the manual loop command quotes the task and carries the choices", () => {
  const request = buildRunRequest({
    ...form,
    task: "it's a cup",
    provider: "openai",
  });
  expect(manualLoopCommand(request, "http://station:8792")).toBe(
    "pnpm --filter @hebbian/agent start --server http://station:8792 --camera chest,wrist_r --provider openai --model gpt-6.2-astra --reasoning-effort high -- follower 'it'\\''s a cup'",
  );
  expect(
    manualLoopCommand(
      buildRunRequest({
        ...form,
        provider: "openai",
        inferenceMode: "ultrafast",
      }),
      "http://station:8792",
    ),
  ).toContain("--reasoning-effort high --inference-mode ultrafast --");
});

test("the status line shows elapsed time against now, or the run's own duration once ended", () => {
  expect(runStatusLine(null, 5000)).toBe("No run yet.");
  expect(runStatusLine(running, 1012)).toBe("running · 12 s");
  expect(
    runStatusLine(
      ended({ kind: "failed", detail: "no key" }, startedAt + 34_000),
      9999,
    ),
  ).toBe("failed · 34 s · no key");
  expect(
    runStatusLine(
      ended({ kind: "incomplete", detail: "no station" }, startedAt + 5000),
      9999,
    ),
  ).toBe("incomplete · 5 s · no station");
});

test("elapsed time formats minutes and hours and never goes negative", () => {
  expect([0, 59.9, 65, 3725, -4].map(formatElapsed)).toEqual([
    "0 s",
    "59 s",
    "1 min 05 s",
    "1 h 02 min",
    "0 s",
  ]);
});

test("output follows the end unless the reader scrolled up", () => {
  expect(shouldFollowOutput(700, 300, 1000)).toBe(true);
  expect(shouldFollowOutput(690, 300, 1000)).toBe(true);
  expect(shouldFollowOutput(100, 300, 1000)).toBe(false);
});

test("chosen lessons apply the operator's edits and switches", () => {
  const lesson = (runId: string, text: string) => ({
    runId,
    outcome: "completed" as const,
    text,
    savedAt: 1,
  });
  const lessons = [
    lesson("a", "approach from above"),
    lesson("b", "open wider"),
    lesson("c", "closest row is nearest the arm"),
  ];
  expect(chosenLessons(lessons, {})).toEqual([
    "approach from above",
    "open wider",
    "closest row is nearest the arm",
  ]);
  expect(
    chosenLessons(lessons, {
      a: { enabled: false, text: "approach from above" },
      b: { enabled: true, text: "  open the gripper wider  " },
      c: { enabled: true, text: "   " },
    }),
  ).toEqual(["open the gripper wider"]);
});
