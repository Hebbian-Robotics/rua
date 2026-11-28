import {
  type ActionSequenceRecord,
  commandPlan,
  commandResult,
  type LiveRun,
} from "hebbian-rua";
import { memo, useEffect, useId, useMemo, useState } from "react";
import { match } from "ts-pattern";
import { Button } from "@/components/ui/button.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs.js";
import type { Run } from "../../../shared/run-api.js";
import { field } from "../../components/assistant-ui/elements/surfaces.js";
import { TodoList } from "../../components/assistant-ui/elements/todo-list.js";
import { ToolCall } from "../../components/assistant-ui/elements/tool-call.js";
import { cn } from "../../lib/utils.js";
import {
  type ClearedRun,
  clearRunContents,
  visibleRunContents,
} from "../../run-visibility.js";
import { spanEnd, spanStart, type TraceSpan } from "../../span-time.js";
import { stationConnection } from "../../viewer-connection.js";

function LiveActionSequence() {
  const [action_sequence, setActionSequence] =
    useState<ActionSequenceRecord | null>(null);
  useEffect(() => stationConnection.onActionSequence(setActionSequence), []);
  if (!action_sequence) return null;
  return (
    <section aria-label="ActionSequence progress">
      <h3>Action sequence · {action_sequence.arms.join(", ")}</h3>
      <p>
        {action_sequence.phase.state === "stopped"
          ? `Stopped: ${action_sequence.phase.failure.detail}`
          : action_sequence.phase.state === "completed"
            ? "Completed"
            : "Running"}
      </p>
      <ol>
        {action_sequence.requested_steps.map((step, index) => {
          const records = action_sequence.steps[index];
          const status = records
            ? records
                .map(({ arm, record }) => {
                  const plan = commandPlan(record);
                  if (plan?.pose_limited)
                    return `${arm}: limited: ${plan.pose_limited.advice}`;
                  if (plan?.path_limited)
                    return `${arm}: limited: ${plan.path_limited.reason}`;
                  return `${arm}: ${
                    commandResult(record)?.outcome.replaceAll("_", " ") ??
                    "interrupted"
                  }`;
                })
                .join(", ")
            : action_sequence.phase.state === "running" &&
                action_sequence.phase.step === index
              ? "running"
              : action_sequence.phase.state === "stopped"
                ? "not executed"
                : "pending";
          const action = Object.entries(step.arms)
            .map(([arm, armAction]) => {
              const move =
                "goal" in armAction
                  ? armAction.goal.kind === "pose"
                    ? "Move to pose"
                    : "Move joints"
                  : "";
              return `${arm}: ${[
                move,
                armAction.gripper_opening != null
                  ? `Gripper ${(armAction.gripper_opening * 100).toFixed(0)}%`
                  : "",
              ]
                .filter(Boolean)
                .join(" + ")}`;
            })
            .join("; ");
          return (
            // biome-ignore lint/suspicious/noArrayIndexKey: ActionSequence steps have stable order and no separate IDs.
            <li key={`${index}-${action}`}>
              {action} — {status}
            </li>
          );
        })}
      </ol>
    </section>
  );
}

function stringList(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

function Usage({
  usage,
  spans,
  time,
}: {
  usage: LiveRun["usage"];
  spans: TraceSpan[];
  time: number;
}) {
  if (!usage) return <p>No model usage reported at this time.</p>;
  const modelCalls = spans.filter(
    (span) =>
      span.attributes["hebbian.kind"] === "model" &&
      typeof span.attributes["gen_ai.usage.input_tokens"] === "number" &&
      spanEnd(span) <= time,
  );
  const prices = modelCalls.flatMap((span) => {
    const price = span.attributes["hebbian.estimated_cost_usd"];
    return typeof price === "number" && Number.isFinite(price) && price >= 0
      ? [price]
      : [];
  });
  const missingPrices = Math.max(0, usage.turns - prices.length);
  const estimate = prices.reduce((total, price) => total + price, 0);
  const cost =
    prices.length === 0
      ? "Cost estimate unavailable"
      : `${missingPrices > 0 ? "At least " : "Estimated "}$${estimate.toFixed(4)}${missingPrices > 0 ? ` (${missingPrices} unpriced)` : ""}`;
  return (
    <p>
      {usage.turns} model call{usage.turns === 1 ? "" : "s"} · Input{" "}
      {usage.input_tokens.toLocaleString()} (
      {usage.cached_input_tokens.toLocaleString()} cached) · Output{" "}
      {usage.output_tokens.toLocaleString()} (includes{" "}
      {usage.reasoning_tokens.toLocaleString()} reasoning) · {cost}
    </p>
  );
}

function RunMemory({
  memory,
  showTask,
}: {
  memory: LiveRun["memory"] | null;
  showTask: boolean;
}) {
  return (
    <div className="run-memory">
      {showTask && memory?.task && <p className="agent-task">{memory.task}</p>}
      <div className="memory-columns">
        <section className="agent-card">
          {memory?.plan?.length ? (
            <TodoList
              title="Plan"
              items={memory.plan.map((step, index) => ({
                id: `${index}-${step.text}`,
                text: step.text,
                status: step.status,
              }))}
            />
          ) : (
            <>
              <h3>Plan</h3>
              <p>No plan yet.</p>
            </>
          )}
        </section>
        <section className="agent-card">
          <div className="agent-card-heading">
            <h3>Notes</h3>
          </div>
          {memory?.notes.length ? (
            <ul>
              {memory.notes.map((note, index) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: Notes have no IDs and carry no local state.
                <li key={`${index}-${note}`}>{note}</li>
              ))}
            </ul>
          ) : (
            <p>No notes yet.</p>
          )}
        </section>
      </div>
    </div>
  );
}

function RunError({ title, detail }: { title: string; detail: string }) {
  return (
    <div role="alert">
      <details className={cn(field, "rounded-2xl px-4 py-3 text-sm")}>
        <summary className="cursor-pointer font-medium text-red-500">
          {title}
        </summary>
        <p className="mt-2 whitespace-pre-wrap break-words text-muted-foreground">
          {detail}
        </p>
      </details>
    </div>
  );
}

function RunRequestDetails({ run }: { run: Run }) {
  const { request } = run;
  return (
    <article aria-label="Run prompt and settings" className="run-overview">
      <p className="run-prompt">{request.task}</p>
      {request.attachmentFilenames?.length ? (
        <p>Attached images: {request.attachmentFilenames.join(", ")}</p>
      ) : null}
      <details className="run-settings">
        <summary>
          Run settings <span>{request.settings.model}</span>
        </summary>
        <dl className="run-settings-list">
          {[
            ["Provider", request.settings.provider],
            ["Model", request.settings.model],
            ["Reasoning effort", request.settings.reasoningEffort],
            ["Inference mode", request.settings.inferenceMode],
            ["Arm", request.arm],
            ["Required cameras", request.cameras.join(", ") || "None"],
            ["Optional cameras", request.optionalCameras.join(", ") || "None"],
          ].map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </details>
    </article>
  );
}

function RunCompletion({ run }: { run: Run }) {
  const { phase } = run;
  const completion =
    phase.state === "ended"
      ? match(phase.outcome)
          .with({ kind: "failed" }, ({ detail }) => (
            <RunError title="Run failed" detail={detail} />
          ))
          .with({ kind: "incomplete" }, ({ detail }) => (
            <RunError title="Run incomplete" detail={detail} />
          ))
          .with({ kind: "completed" }, () =>
            phase.response ? (
              <article
                aria-label="Agent response"
                className={cn(field, "rounded-2xl px-4 py-3 text-sm")}
              >
                <h4 className="mb-2 font-medium">Assistant</h4>
                <p className="whitespace-pre-wrap break-words">
                  {phase.response}
                </p>
              </article>
            ) : (
              <p className="text-sm text-muted-foreground">Run completed.</p>
            ),
          )
          .with({ kind: "stopped" }, () => (
            <p className="text-sm text-muted-foreground">Run stopped.</p>
          ))
          .with({ kind: "interrupted" }, () => (
            <p className="text-sm text-muted-foreground">Run interrupted.</p>
          ))
          .exhaustive()
      : null;
  return (
    <>
      {completion}
      {run.persistenceError && (
        <RunError title="Run storage failed" detail={run.persistenceError} />
      )}
    </>
  );
}

function ActivityTiming({ span }: { span: TraceSpan }) {
  const startedAt = new Date(spanStart(span) * 1000);
  const durationSeconds = Math.max(0, spanEnd(span) - spanStart(span));
  const running = span.attributes["hebbian.active"] === true;
  return (
    <span className="flex shrink-0 flex-col gap-1 text-xs text-muted-foreground tabular-nums">
      <time
        dateTime={startedAt.toISOString()}
        title={startedAt.toLocaleString()}
      >
        {startedAt.toLocaleTimeString()}
      </time>
      <span>{`${durationSeconds.toFixed(1)} s ${running ? "elapsed" : "duration"}`}</span>
    </span>
  );
}

function AgentToolEntry({
  span,
  onInspect,
}: {
  span: TraceSpan;
  onInspect: () => void;
}) {
  const [open, setOpen] = useState(false);
  const toolName = String(span.attributes["gen_ai.tool.name"] ?? span.name);
  const running = span.attributes["hebbian.active"] === true;
  const argumentsValue = span.attributes["gen_ai.tool.call.arguments"];
  const resultValue = span.attributes["gen_ai.tool.call.result"];
  return (
    <div className="activity-tool-row">
      <ActivityTiming span={span} />
      <ToolCall
        label="Tool"
        activeLabel="Running"
        query={toolName}
        request={
          typeof argumentsValue === "string" ? argumentsValue : "Not available"
        }
        result={
          typeof resultValue === "string"
            ? resultValue
            : running
              ? "Running…"
              : "Not available"
        }
        running={running}
        failed={span.status.code.includes("ERROR")}
        open={open}
        onOpenChange={setOpen}
      />
      {open && (
        <Button variant="ghost" size="xs" onClick={onInspect}>
          View in trace
        </Button>
      )}
    </div>
  );
}

export const RunPanels = memo(function RunPanels({
  agentRun: currentAgentRun = null,
  spans: currentSpans,
  memory: currentMemory,
  usage: currentUsage,
  time,
  onSeek,
  live,
  traceOnly = false,
  showRunDetails = false,
}: {
  agentRun?: Run | null;
  spans: TraceSpan[];
  memory: LiveRun["memory"] | null;
  usage: LiveRun["usage"];
  time: number;
  onSeek: (time: number) => void;
  live: boolean;
  traceOnly?: boolean;
  showRunDetails?: boolean;
}) {
  const traceWindowId = useId();
  const [tab, setTab] = useState<"activity" | "trace">(
    traceOnly ? "trace" : "activity",
  );
  const [collapseGeneration, setCollapseGeneration] = useState(0);
  const [zoom, setZoom] = useState(30);
  const [selectedSpan, setSelectedSpan] = useState<string | null>(null);
  const [clearedRun, setClearedRun] = useState<ClearedRun | null>(null);
  const contents = {
    agentRun: currentAgentRun,
    spans: currentSpans,
    memory: currentMemory,
    usage: currentUsage,
  };
  const { agentRun, spans, memory, usage } = visibleRunContents(
    contents,
    clearedRun,
  );
  const hasRunContents = Boolean(
    agentRun ||
      spans.length ||
      memory?.task ||
      memory?.plan?.length ||
      memory?.notes.length ||
      usage,
  );
  const [following, setFollowing] = useState(true);
  const [inspectedTime, setInspectedTime] = useState(time);
  const selectedTime = live && following ? time : live ? inspectedTime : time;
  const displaySpans = useMemo(
    () =>
      spans.map((span) =>
        live && span.attributes["hebbian.active"] === true
          ? { ...span, end_time_unix_nano: (time * 1e9).toFixed(0) }
          : span,
      ),
    [spans, live, time],
  );
  const runStart = useMemo(
    () =>
      displaySpans.length
        ? displaySpans.reduce(
            (earliest, span) => Math.min(earliest, spanStart(span)),
            Number.POSITIVE_INFINITY,
          )
        : time,
    [displaySpans, time],
  );
  const runEnd = useMemo(
    () =>
      displaySpans.reduce(
        (latest, span) => Math.max(latest, spanEnd(span)),
        time,
      ),
    [displaySpans, time],
  );
  const start =
    zoom === 0 ? runStart : Math.max(runStart, selectedTime - zoom / 2);
  const end = zoom === 0 ? runEnd : Math.min(runEnd, selectedTime + zoom / 2);
  const duration = Math.max(0.001, end - start);
  const visibleSpans = displaySpans.filter(
    (span) => spanEnd(span) >= start && spanStart(span) <= end,
  );
  const selected = displaySpans.find((span) => span.span_id === selectedSpan);
  const agentActivity = displaySpans
    .filter(
      (span) =>
        (span.attributes["hebbian.kind"] === "model" &&
          stringList(span.attributes["hebbian.reasoning_summaries"]).length >
            0) ||
        span.attributes["hebbian.kind"] === "tool",
    )
    .sort((a, b) => spanStart(a) - spanStart(b));
  const visibleRun =
    agentRun && (agentRun.phase.state === "ended" || agentRun.persistenceError)
      ? agentRun
      : null;
  function inspect(timeToInspect: number, span: TraceSpan): void {
    setSelectedSpan(span.span_id);
    setInspectedTime(timeToInspect);
    if (live) setFollowing(false);
    else onSeek(timeToInspect);
  }
  return (
    <section className="run-panels" aria-label="Run details">
      <header>
        <div className="run-heading">
          <h2>Run</h2>
          <Button
            variant="outline"
            disabled={!hasRunContents}
            onClick={() => {
              setClearedRun(clearRunContents(contents));
              setSelectedSpan(null);
              setFollowing(true);
            }}
          >
            {traceOnly ? "Clear trace" : "Clear activity"}
          </Button>
        </div>
        {usage && <Usage usage={usage} spans={spans} time={time} />}
      </header>
      {traceOnly && agentRun && (
        <div className="run-content">
          <RunRequestDetails run={agentRun} />
          {visibleRun && <RunCompletion run={visibleRun} />}
        </div>
      )}
      {showRunDetails && (
        <div className="run-content">
          <section aria-label="Plan and arm actions">
            {live && hasRunContents && <LiveActionSequence />}
            {memory && <RunMemory memory={memory} showTask={false} />}
          </section>
        </div>
      )}
      <Tabs
        value={tab}
        onValueChange={(value) => {
          if (value === "activity" || value === "trace") setTab(value);
        }}
      >
        {!traceOnly && (
          <TabsList
            aria-label="Run details"
            activateOnFocus
            variant="line"
            className="mx-4"
          >
            <TabsTrigger value="activity" className="px-4">
              Agent activity
            </TabsTrigger>
            <TabsTrigger value="trace" className="px-4">
              Trace
            </TabsTrigger>
          </TabsList>
        )}
        {!traceOnly && (
          <TabsContent value="activity" className="run-content">
            <section aria-label="Agent activity">
              {agentRun && <RunRequestDetails run={agentRun} />}
              {live && hasRunContents && <LiveActionSequence />}
              {memory && <RunMemory memory={memory} showTask={!agentRun} />}
              <div className="agent-feed">
                <h3>Activity</h3>
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  onClick={() => setCollapseGeneration((value) => value + 1)}
                >
                  Collapse all
                </Button>
                {agentActivity.length === 0 && visibleRun === null ? (
                  <p>No agent activity yet.</p>
                ) : (
                  agentActivity.map((span) =>
                    span.attributes["hebbian.kind"] === "tool" ? (
                      <AgentToolEntry
                        key={`${span.span_id}:${collapseGeneration}`}
                        span={span}
                        onInspect={() => {
                          inspect(spanStart(span), span);
                          setTab("trace");
                        }}
                      />
                    ) : (
                      <details
                        className="reasoning-entry"
                        key={`${span.span_id}:${collapseGeneration}`}
                      >
                        <summary>
                          <strong>Reasoning</strong>
                          <ActivityTiming span={span} />
                          <span className="reasoning-preview">
                            {
                              stringList(
                                span.attributes["hebbian.reasoning_summaries"],
                              )[0]
                            }
                          </span>
                          {span.attributes["hebbian.active"] === true && (
                            <span>Running</span>
                          )}
                          {span.status.code.includes("ERROR") && (
                            <span className="activity-error">Failed</span>
                          )}
                        </summary>
                        <div className="activity-details">
                          {stringList(
                            span.attributes["hebbian.reasoning_summaries"],
                          ).map((summary, index) => (
                            // biome-ignore lint/suspicious/noArrayIndexKey: Summaries have no IDs and carry no local state.
                            <p key={`${index}-${summary}`}>{summary}</p>
                          ))}
                          <Button
                            variant="outline"
                            onClick={() => {
                              inspect(spanStart(span), span);
                              setTab("trace");
                            }}
                          >
                            Inspect at {(spanStart(span) - runStart).toFixed(1)}{" "}
                            s
                          </Button>
                        </div>
                      </details>
                    ),
                  )
                )}
                {visibleRun && <RunCompletion run={visibleRun} />}
              </div>
            </section>
          </TabsContent>
        )}
        <TabsContent value="trace" className="run-content">
          <section aria-label="Trace">
            <div className="trace-controls">
              <label htmlFor={traceWindowId}>
                Window
                <NativeSelect
                  id={traceWindowId}
                  value={zoom}
                  onChange={(event) =>
                    setZoom(Number(event.currentTarget.value))
                  }
                >
                  <NativeSelectOption value={0}>Whole run</NativeSelectOption>
                  <NativeSelectOption value={120}>
                    2 min around playhead
                  </NativeSelectOption>
                  <NativeSelectOption value={30}>
                    30 s around playhead
                  </NativeSelectOption>
                  <NativeSelectOption value={10}>
                    10 s around playhead
                  </NativeSelectOption>
                </NativeSelect>
              </label>
              {live && !following && (
                <Button variant="outline" onClick={() => setFollowing(true)}>
                  Resume live
                </Button>
              )}
            </div>
            {visibleSpans.length === 0 ? (
              <p>Waiting for a model run or robot command.</p>
            ) : (
              <div className="trace-lanes">
                {visibleSpans.map((span) => {
                  const left = Math.max(
                    0,
                    ((spanStart(span) - start) / duration) * 100,
                  );
                  const right = Math.min(
                    100,
                    ((spanEnd(span) - start) / duration) * 100,
                  );
                  return (
                    <div className="trace-row" key={span.span_id}>
                      <span>{span.name}</span>
                      <div className="trace-track">
                        <Button
                          variant="ghost"
                          style={{
                            left: `${left}%`,
                            width: `${Math.max(1, right - left)}%`,
                          }}
                          title={span.name}
                          aria-label={`${span.name}, ${(spanStart(span) - runStart).toFixed(1)} seconds`}
                          onClick={() => inspect(spanStart(span), span)}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
            {selected && (
              <pre className="trace-details">
                {JSON.stringify(
                  {
                    name: selected.name,
                    status: selected.status,
                    attributes: selected.attributes,
                  },
                  null,
                  2,
                )}
              </pre>
            )}
          </section>
        </TabsContent>
      </Tabs>
    </section>
  );
});
