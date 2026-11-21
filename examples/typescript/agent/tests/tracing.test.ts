import { expect, test } from "vitest";
import { Tracer, type WireSpan } from "../src/tracing.js";

test("spans report when they start and when they end, children of the first span", async () => {
  const reported: WireSpan[] = [];
  let now = 1000n;
  const tracer = new Tracer(
    (spans) => reported.push(...spans),
    () => {
      now += 1n;
      return now;
    },
  );
  await tracer.span("root", { a: 1 }, async (root) => {
    await tracer.span("child", {}, async (child) => {
      child.setError("bad");
      expect(child.traceContext().traceparent).toBe(
        `00-${root.traceId}-${child.spanId}-01`,
      );
    });
  });
  const names = reported.map(
    (span) =>
      `${span.name}${span.attributes["hebbian.active"] ? " (running)" : ""}`,
  );
  expect(names).toEqual(["root (running)", "child (running)", "child", "root"]);
  const [root, , child] = [reported[3], reported[1], reported[2]];
  expect(child?.parent_span_id).toBe(root?.span_id);
  expect(child?.status).toEqual({ code: "STATUS_CODE_ERROR", message: "bad" });
  expect(root?.parent_span_id).toBeNull();
  expect(Number(root?.end_time_unix_nano)).toBeGreaterThan(
    Number(root?.start_time_unix_nano),
  );
  expect(root?.trace_id).toMatch(/^[0-9a-f]{32}$/);
  expect(root?.span_id).toMatch(/^[0-9a-f]{16}$/);
});

test("a span that throws ends with an error status", async () => {
  const reported: WireSpan[] = [];
  const tracer = new Tracer((spans) => reported.push(...spans));
  await expect(
    tracer.span("root", {}, async () => Promise.reject(new Error("boom"))),
  ).rejects.toThrow("boom");
  expect(reported.at(-1)?.status).toEqual({
    code: "STATUS_CODE_ERROR",
    message: "boom",
  });
});
