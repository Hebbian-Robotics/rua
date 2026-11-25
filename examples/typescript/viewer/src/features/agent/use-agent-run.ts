import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { match } from "ts-pattern";
import { isRunActive, type Run } from "../../../shared/run-api.js";
import { agentApi } from "../../agent-api.js";
import { errorMessage } from "../../errors.js";
import {
  openRunEventStream,
  type RunStreamRecovery,
  runStreamId,
  runStreamRecovery,
} from "./run-event-stream.js";

const IDLE_POLL_MILLISECONDS = 3000;
const OUTPUT_LINE_LIMIT = 200;

type AgentServerStatus = "disabled" | "checking" | "reachable" | "unreachable";

export function useAgentRun(baseUrl: string, enabled = true) {
  const queryClient = useQueryClient();
  const [recovery, setRecovery] = useState<RunStreamRecovery>("ready");
  const runQuery = useQuery({
    queryKey: ["agent-run", baseUrl],
    queryFn: () => agentApi.currentRun(baseUrl),
    enabled,
    retry: false,
    refetchInterval: (query) => {
      const current = query.state.data ?? null;
      const activeRunId = isRunActive(current) ? (current?.id ?? null) : null;
      return runStreamId(activeRunId, recovery) === null
        ? IDLE_POLL_MILLISECONDS
        : false;
    },
    refetchIntervalInBackground: true,
  });
  const run = enabled ? (runQuery.data ?? null) : null;
  const activeId = runStreamId(
    isRunActive(run) ? (run?.id ?? null) : null,
    recovery,
  );
  const status: AgentServerStatus = !enabled
    ? "disabled"
    : runQuery.isError
      ? "unreachable"
      : runQuery.isPending
        ? "checking"
        : "reachable";
  const providersQuery = useQuery({
    queryKey: ["agent-providers", baseUrl],
    queryFn: () => agentApi.providers(baseUrl),
    enabled: status === "reachable",
    retry: false,
  });
  const setRun = useCallback(
    (value: Run | null | ((previous: Run | null) => Run | null)) => {
      queryClient.setQueryData<Run | null>(
        ["agent-run", baseUrl],
        (previous) =>
          typeof value === "function" ? value(previous ?? null) : value,
      );
    },
    [baseUrl, queryClient],
  );

  // A successful poll after a stream error lets the stream reconnect.
  useEffect(() => {
    if (runQuery.dataUpdatedAt)
      setRecovery(runStreamRecovery("poll_succeeded"));
  }, [runQuery.dataUpdatedAt]);

  useEffect(() => {
    if (activeId === null) return;
    return openRunEventStream(
      `${baseUrl}/api/runs/${activeId}/events`,
      (event) => {
        match(event)
          .with({ type: "snapshot" }, ({ run }) => setRun(run))
          .with({ type: "phase" }, ({ phase }) => {
            setRun((previous) => previous && { ...previous, phase });
          })
          .with({ type: "persistence_error" }, ({ detail }) =>
            setRun(
              (previous) =>
                previous && {
                  ...previous,
                  persistenceError: detail ?? undefined,
                },
            ),
          )
          .with({ type: "output" }, ({ line }) =>
            setRun(
              (previous) =>
                previous && {
                  ...previous,
                  output: [...previous.output, line].slice(-OUTPUT_LINE_LIMIT),
                },
            ),
          )
          .exhaustive();
      },
      () => setRecovery(runStreamRecovery("stream_lost")),
    );
  }, [baseUrl, activeId, setRun]);

  const loadLessons = useCallback(
    (task: string) => agentApi.lessons(baseUrl, task),
    [baseUrl],
  );
  const saveFeedback = useCallback(
    async (id: string, text: string) => {
      const saved = await agentApi.saveFeedback(baseUrl, id, text);
      setRun((previous) => (previous?.id === saved.id ? saved : previous));
      return saved;
    },
    [baseUrl, setRun],
  );

  return {
    run,
    setRun,
    loadLessons,
    saveFeedback,
    status,
    providers: providersQuery.data ?? null,
    connectionError: runQuery.error ? errorMessage(runQuery.error) : "",
    providersError: providersQuery.error
      ? errorMessage(providersQuery.error)
      : "",
  };
}
