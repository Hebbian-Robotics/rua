import { randomUUID } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { context, trace } from "@opentelemetry/api";

interface ModelImage {
  camera: string;
  jpeg: Uint8Array;
  tool_call_id?: string;
}

export class PendingModelImages {
  private readonly directory = process.env["HEBBIAN_RUA_MODEL_INPUTS_DIR"];
  private readonly images: ModelImage[] = [];
  private runDirectory: string | undefined;
  private turn = 0;

  add(
    cameras: Readonly<Record<string, Uint8Array>>,
    toolCallId?: string,
  ): void {
    if (!this.directory) return;
    for (const [camera, jpeg] of Object.entries(cameras))
      this.images.push({
        camera,
        jpeg,
        ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
      });
  }

  async save(): Promise<void> {
    this.turn += 1;
    if (!this.directory || this.images.length === 0) return;
    const images = this.images.splice(0);
    const span = trace.getSpan(context.active());
    const traceId = span?.spanContext().traceId;
    const runDirectory =
      this.runDirectory ?? resolve(this.directory, traceId ?? randomUUID());
    const turnDirectory = join(
      runDirectory,
      `turn-${String(this.turn).padStart(4, "0")}`,
    );
    try {
      await mkdir(turnDirectory, { recursive: true, mode: 0o700 });
      const entries = [];
      for (const [imageIndex, { jpeg, ...source }] of images.entries()) {
        const file = `${imageIndex + 1}.jpg`;
        await writeFile(join(turnDirectory, file), jpeg, {
          flag: "wx",
          mode: 0o600,
        });
        entries.push({ file, ...source });
      }
      const manifest = join(turnDirectory, "index.json");
      await writeFile(
        manifest,
        JSON.stringify({ turn: this.turn, images: entries }, null, 2),
        { flag: "wx", mode: 0o600 },
      );
      span?.setAttribute("hebbian.model_inputs.path", manifest);
      if (this.runDirectory === undefined) {
        this.runDirectory = runDirectory;
        console.error(`Model input images: ${runDirectory}`);
      }
    } catch (error) {
      console.warn("Could not save model input images:", error);
    }
  }
}
