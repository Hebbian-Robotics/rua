// What a running loop shares between its modules.

import type {
  createRobotTools,
  Lease,
  StationConnection,
} from "@hebbian/robot-use-client";
import { StationConnectionError } from "@hebbian/robot-use-client";
import type { TaskMemory } from "./tools.js";
import type { Tracer } from "./tracing.js";

type RobotTools = ReturnType<typeof createRobotTools>;

export interface RunEnvironment {
  connection: StationConnection;
  robot: RobotTools;

  lease: Lease;
  tracer: Tracer;
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
