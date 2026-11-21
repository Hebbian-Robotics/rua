// Durable run summaries and append-only output.
import {
  appendFile,
  mkdir,
  readdir,
  readFile,
  rename,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import { type Run, runSchema } from "../../shared/run-api.js";
import type { RunStore } from "./run-manager.js";

export function fileRunStore(directory: string): RunStore {
  return {
    async loadAll() {
      await mkdir(directory, { recursive: true });
      const runs: Run[] = [];
      for (const entry of await readdir(directory, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const path = join(directory, entry.name, "metadata.json");
        try {
          const run = runSchema.parse(JSON.parse(await readFile(path, "utf8")));
          {
            const output = await readFile(
              join(directory, entry.name, "output.jsonl"),
              "utf8",
            ).catch((error: NodeJS.ErrnoException) => {
              if (error.code === "ENOENT") return "";
              throw error;
            });
            run.output = output
              .split("\n")
              .flatMap((line) => {
                if (!line) return [];
                try {
                  const record: unknown = JSON.parse(line);
                  return typeof record === "object" &&
                    record !== null &&
                    "line" in record &&
                    typeof record.line === "string"
                    ? [record.line]
                    : [];
                } catch {
                  return [];
                }
              })
              .slice(-200);
          }
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
    async readOutput(id) {
      return readFile(join(directory, id, "output.jsonl"), "utf8").catch(
        (error: NodeJS.ErrnoException) => {
          if (error.code === "ENOENT") return "";
          throw error;
        },
      );
    },
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
