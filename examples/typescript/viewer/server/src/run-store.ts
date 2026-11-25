import {
  appendFile,
  mkdir,
  readdir,
  readFile,
  rename,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import {
  RUN_OUTPUT_LINE_LIMIT,
  type Run,
  runSchema,
} from "../../shared/run-api.js";
import type { RunStore } from "./run-manager.js";

const outputRecordSchema = z.object({ line: z.string() });

function recentOutput(records: string): string[] {
  return records
    .split("\n")
    .flatMap((line) => {
      if (!line) return [];
      try {
        // WHY: Older logs and recovery only need the line, even if timing is absent.
        const record = outputRecordSchema.safeParse(JSON.parse(line));
        return record.success ? [record.data.line] : [];
      } catch (error) {
        if (error instanceof SyntaxError) return [];
        throw error;
      }
    })
    .slice(-RUN_OUTPUT_LINE_LIMIT);
}

export function fileRunStore(directory: string): RunStore {
  function readOutput(id: string): Promise<string> {
    return readFile(join(directory, id, "output.jsonl"), "utf8").catch(
      (error: NodeJS.ErrnoException) => {
        if (error.code === "ENOENT") return "";
        throw error;
      },
    );
  }

  return {
    async loadAll() {
      await mkdir(directory, { recursive: true });
      const runs: Run[] = [];
      for (const entry of await readdir(directory, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const path = join(directory, entry.name, "metadata.json");
        try {
          const run = runSchema.parse(JSON.parse(await readFile(path, "utf8")));
          run.output = recentOutput(await readOutput(entry.name));
          runs.push(run);
        } catch (error: unknown) {
          console.error(`Skipping unreadable run record ${entry.name}:`, error);
        }
      }
      return runs;
    },
    async save(run) {
      const runDirectory = join(directory, run.id);
      await mkdir(runDirectory, { recursive: true });
      const path = join(runDirectory, "metadata.json");
      await writeFile(
        `${path}.tmp`,
        JSON.stringify({ ...run, output: [] }, null, 2),
      );
      await rename(`${path}.tmp`, path);
    },
    readOutput,
    async appendOutput(id, line, timestamp) {
      const runDirectory = join(directory, id);
      await mkdir(runDirectory, { recursive: true });
      await appendFile(
        join(runDirectory, "output.jsonl"),
        `${JSON.stringify({ timestamp, line })}\n`,
      );
    },
  };
}
