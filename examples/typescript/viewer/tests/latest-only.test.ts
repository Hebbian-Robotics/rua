import { expect, test } from "vitest";
import { createLatestOnly } from "../src/lib/latest-only.js";

function controllableSender() {
  const sent: number[] = [];
  const settlers: Array<{
    resolve: () => void;
    reject: (error: Error) => void;
  }> = [];
  const send = (value: number) =>
    new Promise<void>((resolve, reject) => {
      sent.push(value);
      settlers.push({ resolve, reject });
    });
  return { sent, settlers, send };
}

test("sends the first value at once and keeps one request in flight", async () => {
  const { sent, settlers, send } = controllableSender();
  const latest = createLatestOnly(send);
  latest.push(1);
  latest.push(2);
  latest.push(3);
  expect(sent).toEqual([1]);
  settlers[0]?.resolve();
  await Promise.resolve();
  await Promise.resolve();
  expect(sent).toEqual([1, 3]);
});

test("the newest value wins and is always eventually sent", async () => {
  const { sent, settlers, send } = controllableSender();
  const latest = createLatestOnly(send);
  latest.push(1);
  latest.push(2);
  settlers[0]?.resolve();
  await Promise.resolve();
  await Promise.resolve();
  latest.push(3);
  settlers[1]?.resolve();
  const idle = latest.idle();
  await Promise.resolve();
  await Promise.resolve();
  settlers[2]?.resolve();
  await idle;
  expect(sent).toEqual([1, 2, 3]);
});

test("an error is reported and does not wedge the queue", async () => {
  const { sent, settlers, send } = controllableSender();
  const errors: unknown[] = [];
  const latest = createLatestOnly(send, (error) => errors.push(error));
  latest.push(1);
  latest.push(2);
  settlers[0]?.reject(new Error("refused"));
  await Promise.resolve();
  await Promise.resolve();
  expect(errors).toHaveLength(1);
  expect(sent).toEqual([1, 2]);
  settlers[1]?.resolve();
  await latest.idle();
  latest.push(4);
  expect(sent).toEqual([1, 2, 4]);
});

test("cancel drops the waiting value but not the one in flight", async () => {
  const { sent, settlers, send } = controllableSender();
  const latest = createLatestOnly(send);
  latest.push(1);
  latest.push(2);
  latest.cancel();
  settlers[0]?.resolve();
  await latest.idle();
  expect(sent).toEqual([1]);
});
