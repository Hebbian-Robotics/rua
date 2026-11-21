import { createRecordingClient } from "@hebbian/robot-use-client";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useEffectEvent, useRef, useState } from "react";
import { z } from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";
import { manualLease } from "../../manual-lease.js";
import type { Run } from "../../run-logic.js";

const recordingClient = createRecordingClient(globalThis.location.href);
const recordingStatusKey = ["recording-status"] as const;

export function useControlRecording() {
  const queryClient = useQueryClient();
  const enabled =
    useQuery({
      queryKey: ["recording-enabled"],
      queryFn: async () =>
        z.boolean().parse(await stationJson("api/recording-enabled")),
      retry: false,
    }).data ?? false;
  const status = useQuery({
    queryKey: recordingStatusKey,
    queryFn: () => recordingClient.status(),
    refetchInterval: 3000,
    refetchIntervalInBackground: true,
    retry: false,
  });
  const name =
    status.data?.state === "recording" || status.data?.state === "stopping"
      ? status.data.name
      : null;
  const [recordWithControl, setRecordWithControl] = useState(false);
  const pendingChange = useRef<Promise<void> | null>(null);
  const change = useMutation({
    mutationFn: async (action: "start" | "stop") => {
      if (action === "stop") {
        await recordingClient.stop();
      } else {
        const recordingName = await recordingClient.start();
        await queryClient.cancelQueries({ queryKey: recordingStatusKey });
        queryClient.setQueryData(recordingStatusKey, {
          state: "recording",
          name: recordingName,
        });
      }
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: recordingStatusKey }),
  });

  function changeRecording(action: "start" | "stop"): Promise<void> {
    if (pendingChange.current) return pendingChange.current;
    if (action === "start" && name) return Promise.resolve();
    const request = change
      .mutateAsync(action)
      .catch(() => {
        // The mutation exposes the failure in the recording controls.
      })
      .finally(() => {
        pendingChange.current = null;
      });
    pendingChange.current = request;
    return request;
  }

  async function startWithControl(): Promise<void> {
    if (enabled && recordWithControl) await changeRecording("start");
  }

  const onManualLeaseChange = useEffectEvent(() => {
    if (manualLease.holdsLease()) void startWithControl();
  });
  useEffect(() => manualLease.onChange(() => onManualLeaseChange()), []);

  const error = change.error
    ? `Could not ${change.variables ?? "change"} recording: ${errorMessage(change.error)}`
    : status.error
      ? `Recording status unavailable: ${errorMessage(status.error)}`
      : status.data?.state === "failed"
        ? `Recording failed: ${status.data.detail}`
        : "";

  return {
    enabled,
    name,
    stopping: status.data?.state === "stopping",
    pending: change.isPending,
    error,
    recordWithControl,
    setRecordWithControl,
    changeRecording,
    startWithControl,
  };
}

export function RecordingControls({
  recording,
  run,
  runActive,
}: {
  recording: ReturnType<typeof useControlRecording>;
  run: Run | null;
  runActive: boolean;
}) {
  const runFailure = run?.output.find((line) =>
    /recording .* failed:/i.test(line),
  );
  if (!recording.enabled && !recording.error && !runFailure) return null;

  return (
    <section className="grid gap-3 px-4 py-3" aria-label="Recording">
      {runFailure && <Alert variant="destructive">{runFailure}</Alert>}
      {recording.error && (
        <Alert variant="destructive">{recording.error}</Alert>
      )}
      {recording.enabled && (
        <>
          <label
            className="flex items-center gap-2"
            htmlFor="record-with-control"
          >
            <Switch
              id="record-with-control"
              checked={recording.recordWithControl}
              disabled={runActive || recording.pending}
              onCheckedChange={recording.setRecordWithControl}
            />
            Record when control starts
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              disabled={recording.pending || recording.stopping}
              onClick={() =>
                void recording.changeRecording(
                  recording.name ? "stop" : "start",
                )
              }
            >
              {recording.name ? "Stop recording" : "Record"}
            </Button>
            <span role="status">
              {recording.stopping
                ? "Stopping recording…"
                : recording.name
                  ? "Recording"
                  : null}
            </span>
          </div>
        </>
      )}
    </section>
  );
}
