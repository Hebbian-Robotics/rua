import type { RobotToolCall, RobotToolResult } from "hebbian-rua";
import { vi } from "vitest";
import {
  commandRecord,
  stateMessage,
} from "../../../../typescript-client/tests/station-fakes.js";
import type {
  ModelConversation,
  ModelRefusal,
  ModelTurn,
  Observation,
} from "../src/providers/types.js";
import type { WireSpan } from "../src/tracing.js";

const MAXIMUM_PUMPS = 5000;
const STATION_INSTANCE = "11111111-1111-4111-8111-111111111111";

export interface SentCommand {
  arm: string;
  command: { kind: string };
  trace_context?: { traceparent: string };
}

export class ScriptedStation {
  armNames = ["follower"];
  revision = 0;
  armMode = "holding";
  events: Record<string, unknown>[] = [];
  spans: WireSpan[] = [];
  commands: SentCommand[] = [];

  requests: string[] = [];
  returned = false;

  leaseStops: string[] = [];
  private completions = new Map<string, unknown>();
  private lastCommandId: string | undefined;
  private stream: ReadableStreamDefaultController<Uint8Array> | null = null;
  private eventSequence = 0;

  install(): void {
    vi.stubGlobal("fetch", async (input: string | URL, init?: RequestInit) =>
      this.route(new URL(input), init),
    );
  }

  private emit(kind: string, payload: unknown): void {
    this.eventSequence += 1;
    try {
      this.stream?.enqueue(
        new TextEncoder().encode(
          `id: ${STATION_INSTANCE}:${this.eventSequence}\nevent: ${kind}\ndata: ${JSON.stringify(payload)}\n\n`,
        ),
      );
    } catch {
      this.stream = null;
    }
  }

  private stationStateMessage() {
    return stateMessage(
      this.armNames.map((arm) => ({
        arm,
        mode: this.armMode,
        sampledAt: 1,
        revision: this.revision,
        ...(this.lastCommandId ? { lastCommandId: this.lastCommandId } : {}),
      })),
    );
  }

  emitState(): void {
    this.emit("station", this.stationStateMessage());
  }

  loseLease(): void {
    this.emit("station", { kind: "lease_lost", detail: "superseded" });
  }

  async run<Result>(work: Promise<Result>): Promise<Result> {
    let finished = false;
    const settled = work.finally(() => {
      finished = true;
    });
    for (let pumps = 0; !finished; pumps += 1) {
      if (pumps > MAXIMUM_PUMPS) throw new Error("The loop did not finish");
      await new Promise<void>((resolve) => setImmediate(resolve));
      this.emitState();
    }
    return settled;
  }

  eventsOfType(type: string): Record<string, unknown>[] {
    return this.events.filter((event) => event["type"] === type);
  }

  finishedSpans(): WireSpan[] {
    return this.spans.filter(
      (span) => span.attributes["hebbian.active"] !== true,
    );
  }

  private route(url: URL, init?: RequestInit): Response {
    const method = init?.method ?? "GET";
    this.requests.push(`${method} ${url.pathname}`);
    const path = url.pathname;
    const body = init?.body
      ? (JSON.parse(String(init.body)) as Record<string, unknown>)
      : {};
    if (path === "/events") {
      const stream = new ReadableStream<Uint8Array>({
        start: (controller) => {
          this.stream = controller;
          this.emit("snapshot", {
            station_instance_id: STATION_INSTANCE,
            sequence: 0,
            run_sequence: 0,
            items: [this.stationStateMessage()],
          });
        },
        cancel: () => {
          this.stream = null;
        },
      });
      init?.signal?.addEventListener(
        "abort",
        () => {
          try {
            this.stream?.close();
          } catch {
            /* WHY: already closed */
          }
          this.stream = null;
        },
        { once: true },
      );
      return new Response(stream, {
        headers: { "Content-Type": "text/event-stream" },
      });
    }
    if (path === "/instance")
      return Response.json({
        id: STATION_INSTANCE,
        supports_operation_completion: true,
      });
    if (path === "/leases") {
      const request = body["request"] as Record<string, unknown>;
      return Response.json({
        lease_token: "token",
        lease_seconds: 15,
        status: {
          holder: "agent",
          label: request["label"],
          required_cameras: request["required_cameras"],
          on_interruption: request["on_interruption"],
          expires_in_seconds: 15,
        },
      });
    }
    if (path === "/leases/release") {
      this.returned = true;
      return new Response(null, { status: 204 });
    }
    if (path === "/leases/renew" || path === "/leases/camera")
      return new Response(null, { status: 204 });
    if (path === "/pose_previews") {
      const poses = body["poses"] as { id: string }[];
      return Response.json({
        results: poses.map((candidate) => ({
          id: candidate.id,
          label: null,
          answer: {
            status: "refused",
            reason: "unreachable",
            detail: "No reachable pose",
          },
        })),
      });
    }
    if (path === "/observations") {
      if (
        body["wait"] &&
        (this.armMode === "disabled" || this.armMode === "faulted")
      ) {
        return Response.json({
          status: "observation_recovery_ended",
          arm: "follower",
          mode: this.armMode,
        });
      }
      const names = (body["required_cameras"] ?? []) as string[];
      return Response.json({
        status: "observation_captured",
        state: this.stationStateMessage().state,
        cameras: Object.fromEntries(
          names.map((name) => [
            name,
            {
              jpeg_base64: "AQID",
              captured_at_unix_ns: null,
              retrieved_between_unix_seconds: [0, 0],
            },
          ]),
        ),
        missing_optional_cameras: body["optional_cameras"] ?? [],
      });
    }
    if (path === "/commands") {
      const command = body as unknown as SentCommand;
      this.commands.push(command);
      const id = `command-${this.commands.length}`;
      this.lastCommandId = id;
      this.emitState();
      const operationId = String(body["operation_id"]);
      this.completions.set(operationId, {
        records: [{ arm: command.arm, record: commandRecord(id, 2) }],
        state_sequence: String(this.eventSequence),
      });
      return Response.json({
        status: "accepted",
        records: [{ arm: command.arm, record: commandRecord(id) }],
      });
    }
    const completion = path.match(/^\/operations\/([^/]+)\/completion$/);
    if (completion) {
      const outcome = this.completions.get(completion[1] ?? "");
      return outcome
        ? Response.json(outcome)
        : new Response(null, { status: 404 });
    }
    const stop = path.match(/^\/arms\/([^/]+)\/stop$/);
    if (stop) {
      this.leaseStops.push(decodeURIComponent(stop[1] ?? ""));
      return new Response(null, { status: 204 });
    }
    if (path === "/run/events") {
      this.events.push(body);
      return new Response(null, { status: 204 });
    }
    if (path === "/run/spans") {
      this.spans.push(...(body["spans"] as WireSpan[]));
      return new Response(null, { status: 204 });
    }
    if (path === "/recording/status") return Response.json({ state: "idle" });
    if (path === "/recording/ensure") return Response.json("recording-1");
    if (path === "/recording/stop-for-lease") return Response.json({});
    if (path.startsWith("/cameras/"))
      return new Response(new Uint8Array([1, 2, 3]));
    return new Response("not found", { status: 404 });
  }
}

type Step =
  | ModelTurn
  | ModelRefusal
  | Error
  | ((conversation: ScriptedConversation) => ModelTurn | ModelRefusal);

export type Received =
  | {
      kind: "results";
      calls: RobotToolCall[];
      texts: string[];
      errors: boolean[];
      observation: Observation | undefined;
    }
  | { kind: "observation"; observation: Observation };

export class ScriptedConversation implements ModelConversation {
  readonly received: Received[] = [];
  private position = 0;

  constructor(private readonly steps: Step[]) {}

  async nextTurn(): Promise<ModelTurn | ModelRefusal> {
    const step = this.steps[this.position];
    this.position += 1;
    if (step === undefined) throw new Error("The script ran out of turns");
    if (step instanceof Error) throw step;
    return typeof step === "function" ? step(this) : step;
  }

  addToolResults(results: RobotToolResult[], observation?: Observation): void {
    this.received.push({
      kind: "results",
      calls: results.map((result) => result.call),
      texts: results.map((result) => result.text),
      errors: results.map((result) => result.isError),
      observation,
    });
  }

  addObservation(observation: Observation): void {
    this.received.push({ kind: "observation", observation });
  }
}

export function turn(...calls: [string, unknown][]): ModelTurn {
  return {
    kind: "turn",
    toolCalls: calls.map(([name, argumentsValue], index) => ({
      id: `call-${name}-${index}`,
      name,
      arguments: argumentsValue,
    })),
    reasoningSummaries: [],
    usage: {
      model: "scripted",
      inputTokens: 10,
      cachedInputTokens: 4,
      outputTokens: 5,
      reasoningTokens: 2,
      cacheWriteTokens: 0,
    },
  };
}
