import { conciseOutput, modelResponse } from "./response-format.js";
import type {
  RobotToolCall,
  SequenceImageHistory,
} from "./robot-tool-types.js";

/** Station data and context; transforms preserve SI units until final rendering. */
export type RobotOutput =
  | {
      readonly kind: "result";
      readonly data: unknown;
      readonly call: RobotToolCall;
      readonly cameras?: Record<string, Uint8Array>;
      readonly sequenceImages?: SequenceImageHistory;
    }
  | {
      readonly kind: "observation";
      readonly data: unknown;
      readonly selectedArm: string;
      readonly notes: readonly string[];
      readonly cameras: Record<string, Uint8Array>;
      readonly sequenceImages?: SequenceImageHistory;
      readonly missingOptionalCameras: readonly string[];
    };

export interface ModelContent {
  text: string;
  cameras?: Record<string, Uint8Array>;
}

/** Formatters select presentation; they cannot change execution or outcome flags. */
export type OutputFormatter = (output: RobotOutput) => ModelContent;

/** Select concise payload fields, preserving SI units and observation context. */
export function summarizeOutput(output: RobotOutput): RobotOutput {
  return { ...output, data: conciseOutput(output.data) };
}

/** Select model images without changing capture or hiding availability notices. */
export function selectOutputCameras(
  output: RobotOutput,
  names: readonly string[],
): RobotOutput {
  const selectedNames = new Set(names);
  return {
    ...output,
    ...(output.sequenceImages
      ? {
          sequenceImages: {
            ...output.sequenceImages,
            images: output.sequenceImages.images.filter((image) =>
              selectedNames.has(image.camera),
            ),
          },
        }
      : {}),
    cameras: Object.fromEntries(
      Object.entries(output.cameras ?? {}).filter(([name]) =>
        selectedNames.has(name),
      ),
    ),
  };
}

/** Render once, converting to readable model units or retaining exact SI data. */
export function renderOutput(
  output: RobotOutput,
  options: { units?: "model" | "si" } = {},
): ModelContent {
  const data =
    options.units === "si" ? output.data : modelResponse(output.data);
  const text = typeof data === "string" ? data : JSON.stringify(data);
  const historicalCameras = Object.fromEntries(
    (output.sequenceImages?.images ?? []).map((image, index) => [
      `Sequence ${output.sequenceImages?.callId} image ${index + 1}: ${image.camera}, +${image.elapsedSeconds.toFixed(3)}s, ${image.boundary}; retrieved ${image.retrievedBetweenUnixSeconds.join("–")} Unix seconds`,
      image.jpeg,
    ]),
  );
  if (output.kind === "result")
    return { text, cameras: { ...output.cameras, ...historicalCameras } };
  return {
    text: [
      ...(output.sequenceImages
        ? [
            `Visual history for action_sequence call ${output.sequenceImages.callId}: sampled images, not synchronized robot state. Missing samples: ${output.sequenceImages.missedSamples}; retention truncated: ${output.sequenceImages.truncated}. Images named for the sequence are historical; plain camera names are current.`,
          ]
        : []),
      ...output.notes,
      `Selected arm: ${output.selectedArm}.`,
      `Arm state: ${text}`,
      ...(output.missingOptionalCameras.length > 0
        ? [
            `Optional cameras unavailable: ${output.missingOptionalCameras.join(", ")}`,
          ]
        : []),
    ].join("\n"),
    cameras: { ...output.cameras, ...historicalCameras },
  };
}

/** Concise output in readable model units, retaining the configured camera images. */
export function defaultFormatter(output: RobotOutput): ModelContent {
  return renderOutput(summarizeOutput(output));
}

/** Complete payload in original SI units and precision. */
export function fullFormatter(output: RobotOutput): ModelContent {
  return renderOutput(output, { units: "si" });
}

/** Interpret selected model text as JSON when possible, otherwise retain the text. */
export function parseModelData(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}
