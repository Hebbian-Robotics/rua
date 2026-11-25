import type { TracerProvider } from "@opentelemetry/api";
import type { createRobotTools, Lease, StationConnection } from "hebbian-rua";
import { StationConnectionError } from "hebbian-rua";
import type { TaskMemory } from "./tools.js";
import type { SpanRunner } from "./tracing.js";

type RobotTools = ReturnType<typeof createRobotTools>;

export interface RunEnvironment {
  connection: StationConnection;
  robot: RobotTools;

  lease: Lease;
  withSpan: SpanRunner;
  tracerProvider: TracerProvider;
  memory: TaskMemory;
  arm: string;

  cameras: readonly string[];

  leaseLost: () => boolean;

  print: (line: string) => void;
}

export function reportEvent(
  connection: StationConnection,
  event: Record<string, unknown>,
): void {
  try {
    connection.recordRunEvent(event);
  } catch (error) {
    if (!(error instanceof StationConnectionError)) throw error;
  }
}
