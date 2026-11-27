import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
      request: {
        arm: "arm",
        task: "task",
        settings: {
          provider: "openai",
          model: "example-model",
          reasoningEffort: "high",
          inferenceMode: "standard",
        },
      },
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

test("loads attachment names from runs saved before multiple images were supported", async () => {
  const directory = await mkdtemp(join(tmpdir(), "agent-runs-"));
  try {
    const runDirectory = join(directory, "legacy-run");
    await mkdir(runDirectory);
    await writeFile(
      join(runDirectory, "metadata.json"),
      JSON.stringify({
        id: "legacy-run",
        createdAt: 1,
        request: {
          arm: "arm",
          task: "task",
          attachmentFilename: "reference.jpg",
          settings: {
            provider: "openai",
            model: "example-model",
            reasoningEffort: "high",
            inferenceMode: "standard",
          },
        },
        phase: { state: "running", startedAt: 1 },
        output: [],
      }),
    );
    const loaded = await fileRunStore(directory).loadAll();
    expect(loaded[0]?.request.attachmentFilenames).toEqual(["reference.jpg"]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
