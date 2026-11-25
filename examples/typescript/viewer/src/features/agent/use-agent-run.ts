import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import {
  isRunActive,
  type Provider,
  type Run,
} from "../../../shared/run-api.js";
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

type AgentServerStatus = "checking" | "reachable" | "unreachable";

export function useAgentRun(baseUrl: string) {
  const queryClient = useQueryClient();
  const [recovery, setRecovery] = useState<RunStreamRecovery>("ready");
  const runQuery = useQuery({
    queryKey: ["agent-run", baseUrl],
    queryFn: () => agentApi.currentRun(baseUrl),
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
  const run = runQuery.data ?? null;
  const activeId = runStreamId(
    isRunActive(run) ? (run?.id ?? null) : null,
    recovery,
  );
  const status: AgentServerStatus = runQuery.isError
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
  const [, refreshKeys] = useState(0);
  useEffect(() => {
    const refresh = () => refreshKeys((previous) => previous + 1);
    window.addEventListener("storage", refresh);
    return () => window.removeEventListener("storage", refresh);
  }, []);
  function browserKey(provider: Provider) {
    if (
      providersQuery.data?.some(
        (entry) => entry.provider === provider && entry.available,
      )
    )
      return undefined;
    try {
      return localStorage.getItem(`rua.viewer.keys.${provider}`) ?? undefined;
    } catch {
      return undefined;
    }
  }
  function saveKey(provider: Provider, value: string) {
    const name = `rua.viewer.keys.${provider}`;
    if (value) localStorage.setItem(name, value);
    else localStorage.removeItem(name);
    refreshKeys((previous) => previous + 1);
  }
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
    browserKey,
    saveKey,
    status,
    providers: providersQuery.data ?? null,
    connectionError: runQuery.error ? errorMessage(runQuery.error) : "",
    providersError: providersQuery.error
      ? errorMessage(providersQuery.error)
      : "",
  };
}
