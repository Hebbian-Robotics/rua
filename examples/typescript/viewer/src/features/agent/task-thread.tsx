import {
  MessagePrimitive,
  type ThreadMessageLike,
  ThreadPrimitive,
  type ToolCallMessagePartComponent,
} from "@assistant-ui/react";
import { useState } from "react";
import {
  ReasoningContent,
  ReasoningRoot,
  ReasoningText,
  ReasoningTrigger,
} from "../../components/assistant-ui/elements/reasoning.js";
import { ToolCall } from "../../components/assistant-ui/elements/tool-call.js";
import type { Run } from "../../run-logic.js";
import type { TraceSpan } from "../../span-time.js";
import { spanStart } from "../../span-time.js";

export function taskMessages(
  run: Run | null,
  spans: TraceSpan[],
): ThreadMessageLike[] {
  if (!run) return [];
  const messages: ThreadMessageLike[] = [
    { id: `${run.id}:prompt`, role: "user", content: run.request.task },
  ];
  for (const span of [...spans].sort(
    (left, right) => spanStart(left) - spanStart(right),
  )) {
    const kind = span.attributes["hebbian.kind"];
    if (kind === "model") {
      const summaries = span.attributes["hebbian.reasoning_summaries"];
      if (Array.isArray(summaries)) {
        const text = summaries
          .filter((value): value is string => typeof value === "string")
          .join("\n");
        if (text)
          messages.push({
            id: span.span_id,
            role: "assistant",
            content: [{ type: "reasoning", text }],
          });
      }
    } else if (kind === "tool") {
      const argsText = span.attributes["gen_ai.tool.call.arguments"];
      const result = span.attributes["gen_ai.tool.call.result"];
      const running = span.attributes["hebbian.active"] === true;
      messages.push({
        id: span.span_id,
        role: "assistant",
        content: [
          {
            type: "tool-call",
            toolCallId: span.span_id,
            toolName: String(span.attributes["gen_ai.tool.name"] ?? span.name),
            argsText: typeof argsText === "string" ? argsText : "",
            ...(running ? {} : { result: result ?? "No result available" }),
            isError: span.status.code.includes("ERROR"),
          },
        ],
      });
    }
  }
  if (run.phase.state === "ended") {
    const text =
      run.phase.response ??
      (run.phase.outcome.kind === "completed"
        ? "Task completed."
        : run.phase.outcome.kind === "failed" ||
            run.phase.outcome.kind === "incomplete"
          ? run.phase.outcome.detail
          : `Task ${run.phase.outcome.kind}.`);
    messages.push({ id: `${run.id}:result`, role: "assistant", content: text });
  }
  return messages;
}

function TaskText({ text }: { text: string }) {
  return <p className="whitespace-pre-wrap break-words">{text}</p>;
}
function TaskReasoning({ text }: { text: string }) {
  return (
    <ReasoningRoot variant="ghost">
      <ReasoningTrigger />
      <ReasoningContent>
        <ReasoningText>
          <p className="whitespace-pre-wrap break-words">{text}</p>
        </ReasoningText>
      </ReasoningContent>
    </ReasoningRoot>
  );
}

const TaskTool: ToolCallMessagePartComponent = ({
  toolName,
  argsText,
  result,
  status,
  isError,
}) => {
  const [open, setOpen] = useState(false);
  const running = status.type === "running";
  const resultText =
    result === undefined
      ? running
        ? "Running…"
        : "Not available"
      : typeof result === "string"
        ? result
        : JSON.stringify(result, null, 2);
  return (
    <ToolCall
      label={isError ? "Failed" : "Tool"}
      activeLabel="Running"
      query={toolName}
      request={argsText || "Not available"}
      result={resultText}
      running={running}
      failed={isError}
      open={open}
      onOpenChange={setOpen}
    />
  );
};

function UserTaskMessage() {
  return (
    <MessagePrimitive.Root className="rounded-xl bg-muted p-3">
      <MessagePrimitive.Parts components={{ Text: TaskText }} />
    </MessagePrimitive.Root>
  );
}
function AssistantTaskMessage() {
  return (
    <MessagePrimitive.Root className="grid gap-2">
      <MessagePrimitive.Parts
        components={{
          Text: TaskText,
          Reasoning: TaskReasoning,
          tools: { Fallback: TaskTool },
        }}
      />
    </MessagePrimitive.Root>
  );
}
export function TaskThread() {
  return (
    <ThreadPrimitive.Viewport
      className="agent-thread-viewport grid content-start gap-3"
      autoScroll
    >
      <ThreadPrimitive.Messages
        components={{
          UserMessage: UserTaskMessage,
          AssistantMessage: AssistantTaskMessage,
        }}
      />
    </ThreadPrimitive.Viewport>
  );
}
