import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { match } from "ts-pattern";
import {
  isRunActive,
  type Run,
  runEventSchema,
} from "../../../shared/run-api.js";
import { agentApi } from "../../agent-api.js";
import { errorMessage } from "../../errors.js";

const IDLE_POLL_MILLISECONDS = 3000;
const OUTPUT_LINE_LIMIT = 200;

type AgentServerStatus = "checking" | "reachable" | "unreachable";

export function useAgentRun(baseUrl: string) {
  const queryClient = useQueryClient();
  const [streamFailed, setStreamFailed] = useState(false);
  const runQuery = useQuery({
    queryKey: ["agent-run", baseUrl],
    queryFn: () => agentApi.currentRun(baseUrl),
    retry: false,
    refetchInterval: (query) =>
      isRunActive(query.state.data ?? null) && !streamFailed
        ? false
        : IDLE_POLL_MILLISECONDS,
    refetchIntervalInBackground: true,
  });
  const run = runQuery.data ?? null;
  const activeId = isRunActive(run) ? (run?.id ?? null) : null;
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
    if (runQuery.dataUpdatedAt) setStreamFailed(false);
  }, [runQuery.dataUpdatedAt]);

  useEffect(() => {
    if (activeId === null || streamFailed) return;
    const source = new EventSource(`${baseUrl}/api/runs/${activeId}/events`);
    source.onmessage = (message) => {
      const event = runEventSchema.safeParse(JSON.parse(message.data));
      if (!event.success) return;
      match(event.data)
        .with({ type: "snapshot" }, ({ run }) => setRun(run))
        .with({ type: "phase" }, ({ phase }) => {
          setRun((previous) => previous && { ...previous, phase });
          if (phase.state === "ended") source.close();
        })
        .with({ type: "persistence_error" }, ({ detail }) =>
          setRun(
            (previous) => previous && { ...previous, persistenceError: detail },
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
    };
    source.onerror = () => {
      source.close();
      setStreamFailed(true);
    };
    return () => source.close();
  }, [baseUrl, activeId, streamFailed, setRun]);

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
