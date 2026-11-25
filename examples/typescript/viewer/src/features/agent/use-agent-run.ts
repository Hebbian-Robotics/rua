import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { isRunActive, type Run } from "../../../shared/run-api.js";
import { agentApi } from "../../agent-api.js";
import { errorMessage } from "../../errors.js";
import {
  applyRunEvent,
  openRunEventStream,
  type RunStreamRecovery,
  runStreamId,
  runStreamRecovery,
} from "./run-event-stream.js";

const IDLE_POLL_MILLISECONDS = 3000;

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

  // WHY: A successful poll after a stream error lets the stream reconnect.
  useEffect(() => {
    if (runQuery.dataUpdatedAt)
      setRecovery(runStreamRecovery("poll_succeeded"));
  }, [runQuery.dataUpdatedAt]);

  useEffect(() => {
    if (activeId === null) return;
    return openRunEventStream(
      `${baseUrl}/api/runs/${activeId}/events`,
      (event) => setRun((previous) => applyRunEvent(previous, event)),
      () => setRecovery(runStreamRecovery("stream_lost")),
    );
  }, [baseUrl, activeId, setRun]);

  return {
    run,
    setRun,
    status,
    providers: providersQuery.data ?? null,
    connectionError: runQuery.error ? errorMessage(runQuery.error) : "",
    providersError: providersQuery.error
      ? errorMessage(providersQuery.error)
      : "",
  };
}
