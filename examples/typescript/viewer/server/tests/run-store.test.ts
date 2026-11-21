import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vitest";
import { runSchema } from "../../shared/run-api.js";
import { fileRunStore } from "../src/run-store.js";

test("run output survives restart separately from metadata", async () => {
  const directory = await mkdtemp(join(tmpdir(), "agent-runs-"));
  try {
    const store = fileRunStore(directory);
    const run = runSchema.parse({
      id: "run-1",
      createdAt: 1,
      request: { arm: "arm", task: "task", settings: {} },
      phase: { state: "running", startedAt: 1 },
      output: [],
    });
    await store.save(run);
    await store.appendOutput("run-1", "new line", 2);
    const loaded = await store.loadAll();
    expect(loaded).toHaveLength(1);
    expect(loaded[0]?.output).toEqual(["new line"]);
    expect(
      JSON.parse(
        await readFile(join(directory, "run-1", "metadata.json"), "utf8"),
      ).output,
    ).toEqual([]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
