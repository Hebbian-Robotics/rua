import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createRecordingClient } from "hebbian-rua";
import { useRef } from "react";
import { z } from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";
import type { Run } from "../../run-logic.js";

const recordingClient = createRecordingClient(globalThis.location.href);
const recordingStatusKey = ["recording-status"] as const;

export function useControlRecording(recordWithControl: boolean) {
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
    changeRecording,
    startWithControl,
  };
}

export function RecordingControls({
  recording,
  run,
  autoRecord,
  onAutoRecordChange,
}: {
  recording: ReturnType<typeof useControlRecording>;
  run: Run | null;
  autoRecord: boolean;
  onAutoRecordChange: (enabled: boolean) => void;
}) {
  const runFailure = run?.output.find((line) =>
    /recording .* failed:/i.test(line),
  );
  const error = (recording.enabled && recording.error) || runFailure;

  return (
    <section className="grid gap-2" aria-labelledby="recording-heading">
      <h3 id="recording-heading">Recording</h3>
      <div className="empty:hidden" role="status">
        {error && (
          <Alert role="presentation" variant="destructive">
            {error}
          </Alert>
        )}
      </div>
      <label className="setting-switch" htmlFor="record-with-control">
        <span>Auto-record when control starts</span>
        <Switch
          id="record-with-control"
          checked={autoRecord}
          disabled={!recording.enabled}
          aria-describedby="auto-record-description"
          onCheckedChange={onAutoRecordChange}
        />
      </label>
      <p
        id="auto-record-description"
        className="m-0 text-sm text-muted-foreground"
      >
        {recording.enabled
          ? "Starts when you run a task or enable the arm in Manual."
          : "Recording is unavailable on this station."}
      </p>
      {recording.enabled && (
        <Button
          className="w-fit"
          variant="outline"
          disabled={recording.pending || recording.stopping}
          onClick={() =>
            void recording.changeRecording(recording.name ? "stop" : "start")
          }
        >
          {recording.name ? "Stop recording" : "Record now"}
        </Button>
      )}
    </section>
  );
}
