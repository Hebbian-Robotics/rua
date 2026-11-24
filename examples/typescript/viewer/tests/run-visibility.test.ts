import { expect, test } from "vitest";
import type { Run } from "../shared/run-api.js";
import { clearRunContents, visibleRunContents } from "../src/run-visibility.js";
import type { TraceSpan } from "../src/span-time.js";

function span(trace_id: string, span_id: string): TraceSpan {
  return {
    trace_id,
    span_id,
    parent_span_id: null,
    name: "model",
    start_time_unix_nano: "1000000000",
    end_time_unix_nano: "2000000000",
    attributes: {},
    status: { code: "STATUS_CODE_OK", message: null },
  };
}

test("clearing dismisses every part of a run and its later updates while admitting a new run", () => {
  const agentRun: Run = {
    id: "first",
    createdAt: 1,
    request: {
      arm: "arm",
      task: "Pick up the block",
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
    phase: { state: "running", startedAt: 1 },
    output: [],
  };
  const contents = {
    agentRun,
    spans: [span("first-trace", "first-span")],
    usage: {
      run_id: "first",
      turns: 1,
      input_tokens: 100,
      cached_input_tokens: 0,
      output_tokens: 10,
      reasoning_tokens: 0,
    },
    memory: { task: "Pick up the block", plan: null, notes: ["Block located"] },
  };
  const cleared = clearRunContents(contents);
  const emptyRun = { agentRun: null, spans: [], usage: null, memory: null };
  expect(visibleRunContents(contents, cleared)).toEqual(emptyRun);
  expect(
    visibleRunContents(
      {
        ...contents,
        spans: [...contents.spans, span("first-trace", "later-span")],
        memory: { ...contents.memory, notes: ["Done"] },
      },
      cleared,
    ),
  ).toEqual(emptyRun);
  const nextRun = {
    ...contents,
    agentRun: { ...agentRun, id: "second" },
    usage: { ...contents.usage, run_id: "second" },
    memory: { ...contents.memory, task: "Place the block" },
    spans: [...contents.spans, span("second-trace", "new-span")],
  };
  expect(visibleRunContents(nextRun, cleared)).toEqual({
    ...nextRun,
    spans: [span("second-trace", "new-span")],
  });
});
