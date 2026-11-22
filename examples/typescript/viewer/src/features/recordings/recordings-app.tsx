import { httpRangeReadable } from "@hebbian/mcap-tools/http-range-readable";
import {
  MAXIMUM_CONTINUOUS_PLAYBACK_RATE,
  RecordingClock,
  type SyncedClip,
} from "@hebbian/mcap-tools/playback-clock";
import { useMutation, useQuery } from "@tanstack/react-query";
import { apiSchemas } from "hebbian-rua";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type * as z from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Card, CardContent } from "@/components/ui/card.js";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";
import { recordedRunTelemetry } from "../../recorded-run.js";
import {
  groupRecordingClips,
  type LoadedRecording,
  loadRecording,
  recordingBounds,
} from "../../recording-timeline.js";
import { ConfirmationDialog } from "../../shared/confirmation-dialog.js";
import { ViewerShell } from "../../shared/navigation.js";
import { formatOffset } from "../../shared/time.js";
import { RunPanels } from "../run/run-panels.js";
import type {
  ViewerColorScheme,
  ViewerPreferences,
} from "../settings/preferences.js";
import { useViewerPreferences } from "../settings/use-viewer-preferences.js";
import { JointCharts } from "../telemetry/joint-charts.js";
import { PlaybackVideo } from "./playback-video.js";
import { recordingNavigationLinks } from "./recording-navigation.js";
import "../../app/ui.css";
import "../../app/workspace.css";
import "./recordings.css";

type Recordings = z.infer<typeof apiSchemas.shape.Recordings>;
type Recording = Recordings["recordings"][number];
function recordingTitle(name: string): string {
  const match = /^(\d{13})-[a-f0-9]{32}$/.exec(name);
  if (!match) return name;
  const startedAt = new Date(Number(match[1]));
  return Number.isNaN(startedAt.getTime()) ? name : startedAt.toLocaleString();
}
const RobotScene = lazy(() =>
  import("../robot-scene/robot-scene.js").then(({ RobotScene }) => ({
    default: RobotScene,
  })),
);
function recordingFileUrl(path: string): string {
  return `api/recordings/${path.split("/").map(encodeURIComponent).join("/")}`;
}
function sampleAtOrBefore(times: number[], time: number): number {
  let lower = 0;
  let upper = times.length;
  while (lower < upper) {
    const middle = (lower + upper) >>> 1;
    if ((times[middle] ?? Number.POSITIVE_INFINITY) <= time) lower = middle + 1;
    else upper = middle;
  }
  return lower - 1;
}

function Playback({
  recording,
  preferences,
  colorScheme,
}: {
  recording: LoadedRecording;
  preferences: ViewerPreferences;
  colorScheme: ViewerColorScheme;
}) {
  const { timeline } = recording;
  const clipsByCamera = useMemo(
    () => [...groupRecordingClips(timeline.clips)],
    [timeline.clips],
  );
  const [registered, setRegistered] = useState<Map<string, SyncedClip>>(
    () => new Map(),
  );
  const [clock, setClock] = useState<RecordingClock | null>(null);
  const { start, end } = useMemo(() => recordingBounds(timeline), [timeline]);
  const [time, setTime] = useState(start);
  const [playing, setPlaying] = useState(false);
  const [buffering, setBuffering] = useState(false);
  const [preparingFastPlayback, setPreparingFastPlayback] = useState(false);
  const [speed, setSpeed] = useState(1);
  const lastRender = useRef(0);
  const { memoryUpdates, usageUpdates } = useMemo(
    () => recordedRunTelemetry(timeline),
    [timeline],
  );
  const register = useCallback(
    (path: string, clip: SyncedClip | null) =>
      setRegistered((previous) => {
        const next = new Map(previous);
        if (clip) next.set(path, clip);
        else next.delete(path);
        return next;
      }),
    [],
  );
  useEffect(() => {
    if (registered.size !== timeline.clips.length || end <= start) return;
    const recordingClock = new RecordingClock(start, end, [
      ...registered.values(),
    ]);
    const unsubscribe = recordingClock.onChange(
      (now, isPlaying, isBuffering) => {
        const frame = performance.now();
        if (frame - lastRender.current >= 33 || !isPlaying || isBuffering) {
          setTime(now);
          lastRender.current = frame;
        }
        setPlaying(isPlaying);
        setBuffering(isBuffering);
      },
    );
    setClock(recordingClock);
    if (timeline.clips.length)
      recordingClock.seek(
        timeline.clips.reduce(
          (earliest, clip) => Math.min(earliest, clip.startTime),
          Number.POSITIVE_INFINITY,
        ),
      );
    return () => {
      unsubscribe();
      recordingClock.destroy();
      setClock(null);
    };
  }, [registered, timeline.clips, start, end]);
  useEffect(() => {
    if (!clock) return;
    clock.playbackRate = speed;
    if (speed <= MAXIMUM_CONTINUOUS_PLAYBACK_RATE) {
      setPreparingFastPlayback(false);
      return;
    }
    let active = true;
    setPreparingFastPlayback(true);
    void Promise.all(
      [...registered.values()]
        .filter((clip) => !clip.failed)
        .map((clip) => clip.cacheLocally()),
    ).then(() => {
      if (active) setPreparingFastPlayback(false);
    });
    return () => {
      active = false;
    };
  }, [clock, speed, registered]);
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (
        !clock ||
        event.target instanceof HTMLInputElement ||
        event.target instanceof HTMLTextAreaElement ||
        event.target instanceof HTMLSelectElement ||
        event.target instanceof HTMLButtonElement
      )
        return;
      const step = event.shiftKey ? 5 : 1 / 30;
      if (event.key === " ") clock.togglePlaying();
      else if (event.key === "ArrowLeft") clock.seek(clock.currentTime - step);
      else if (event.key === "ArrowRight") clock.seek(clock.currentTime + step);
      else return;
      event.preventDefault();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [clock]);
  const state = timeline.state;
  const index = state ? sampleAtOrBefore(state.times, time) : -1;
  const arms =
    state?.arms.map((arm) => ({
      arm: arm.arm,
      basePose: arm.basePose,
      measuredJoints: arm.measuredJoints.map((values) => values[index] ?? null),
      commandedJoints: arm.commandedJoints.map(
        (values) => values[index] ?? null,
      ),
      destinationJoints: arm.destinationJoints[index] ?? null,
      destinationLimited: arm.destinationLimited[index] ?? false,
      gripperOpening: arm.gripperOpening[index] ?? null,
    })) ?? [];
  if (end <= start) return <p>This recording has nothing to play.</p>;
  return (
    <>
      <Card className="my-4">
        <CardContent className="grid gap-3">
          <div className="transport-controls">
            <Button
              variant="outline"
              disabled={!clock}
              onClick={() => clock?.togglePlaying()}
            >
              {playing ? "Pause" : "Play"}
            </Button>
            <span>
              {formatOffset(time - start)} / {formatOffset(end - start)}
            </span>
            <label htmlFor="playback-speed">
              Speed
              <NativeSelect
                id="playback-speed"
                value={speed}
                onChange={(event) =>
                  setSpeed(Number(event.currentTarget.value))
                }
              >
                {[0.25, 0.5, 1, 2, 4, 8, 16].map((rate) => (
                  <NativeSelectOption key={rate} value={rate}>
                    {rate}×
                  </NativeSelectOption>
                ))}
              </NativeSelect>
            </label>
            <span>{new Date(time * 1000).toLocaleTimeString()}</span>
            {buffering && <span role="status">Buffering…</span>}
            {preparingFastPlayback && (
              <span role="status">Preparing fast playback…</span>
            )}
          </div>
          <label>
            Recording time
            <input
              type="range"
              min={0}
              max={end - start}
              step="any"
              value={Math.max(0, Math.min(end - start, time - start))}
              onChange={(event) =>
                clock?.seek(start + Number(event.currentTarget.value))
              }
            />
          </label>
        </CardContent>
      </Card>
      <div className="playback-grid">
        {clipsByCamera.map(([camera, clips]) => (
          <PlaybackVideo
            key={camera}
            clips={clips}
            loadVideo={recording.clipVideo}
            register={register}
          />
        ))}
        {state && (
          <Suspense
            fallback={
              <p className="text-muted-foreground">Loading robot view…</p>
            }
          >
            <RobotScene
              colorScheme={colorScheme}
              jointNames={state.jointNames}
              arms={arms}
              showTarget={preferences.showTargetJoints}
              showCommanded={preferences.showCommandedJoints}
              target={null}
              projection={null}
              surfaceGuard={null}
            />
          </Suspense>
        )}
      </div>
      {state && (
        <details className="telemetry-panel">
          <summary>Joint telemetry</summary>
          <JointCharts
            track={state}
            startTime={start}
            currentTime={time}
            showTarget={preferences.showTargetJoints}
            showCommanded={preferences.showCommandedJoints}
            onSeek={(next) => clock?.seek(next)}
          />
        </details>
      )}
      <RunPanels
        spans={timeline.spans}
        memory={
          memoryUpdates.findLast((update) => update.timestamp <= time)
            ?.memory ?? null
        }
        usage={
          usageUpdates.findLast((update) => update.timestamp <= time)?.usage ??
          null
        }
        time={time}
        onSeek={(next) => clock?.seek(next)}
        live={false}
      />
    </>
  );
}

export function RecordingsApp() {
  const [message, setMessage] = useState("");
  const name = new URLSearchParams(location.search).get("name");
  const { preferences, colorScheme } = useViewerPreferences();
  const recordingsQuery = useQuery({
    queryKey: ["recordings"],
    queryFn: async () =>
      apiSchemas.shape.Recordings.parse(await stationJson("api/recordings")),
    retry: false,
  });
  const recordings = recordingsQuery.data ?? null;
  const recording: Recording | undefined = recordings?.recordings.find(
    (entry) => entry.name === name,
  );
  const recordingQuery = useQuery({
    queryKey: ["recording", recording?.mcapPath],
    queryFn: () => {
      if (!recording?.mcapPath)
        throw new Error("This recording has no data file.");
      return loadRecording(
        httpRangeReadable(recordingFileUrl(recording.mcapPath)),
      );
    },
    enabled: Boolean(recording?.mcapPath && recordings?.active !== name),
    staleTime: Infinity,
    retry: false,
  });
  const loaded = recordingQuery.data ?? null;
  const loadMessage =
    recordingsQuery.error || recordingQuery.error
      ? errorMessage(
          recordingsQuery.error ?? recordingQuery.error,
          "Could not load the recording.",
        )
      : recordings && name && !recording
        ? "This recording could not be found."
        : name && recordings?.active === name
          ? "Recording in progress. The synced player opens once it stops."
          : recording && !recording.mcapPath
            ? "This recording has no data file."
            : "";
  const deleteMutation = useMutation({
    mutationFn: async (recordingName: string) => {
      const response = await fetch(
        `api/recordings/${encodeURIComponent(recordingName)}`,
        { method: "DELETE" },
      );
      if (!response.ok)
        throw new Error(`Delete failed (HTTP ${response.status}).`);
    },
    onSuccess: () => location.assign("recording.html"),
    onError: (error) => setMessage(errorMessage(error)),
  });
  function deleteRecording(): void {
    if (
      !recording ||
      recordings?.active === recording.name ||
      deleteMutation.isPending
    )
      return;
    deleteMutation.mutate(recording.name);
  }
  return (
    <ViewerShell
      current="recordings"
      navigationLinks={recordingNavigationLinks}
    >
      <main className="recordings-page">
        <header className="recordings-heading">
          <div>
            <h1>{name ? recordingTitle(name) : "Recordings"}</h1>
            <p>
              {name ? (
                <a href="recording.html">All recordings</a>
              ) : (
                "Review robot recordings."
              )}
            </p>
          </div>
          {recording && (
            <div className="recording-actions">
              {recording.mcapPath && (
                <a
                  href={recordingFileUrl(recording.mcapPath)}
                  download={`${recording.name}.mcap`}
                >
                  Download recording MCAP
                </a>
              )}
              <ConfirmationDialog
                trigger={
                  <Button
                    variant="outline"
                    disabled={
                      deleteMutation.isPending ||
                      recordings?.active === recording.name
                    }
                  >
                    {deleteMutation.isPending
                      ? "Deleting…"
                      : "Delete recording"}
                  </Button>
                }
                title={`Delete recording ${recordingTitle(recording.name)}?`}
                description="This permanently deletes the recording and its files."
                actionLabel="Delete recording"
                onConfirm={deleteRecording}
              />
            </div>
          )}
        </header>
        {(message || loadMessage) && (
          <Alert role="status" className="my-4">
            {message || loadMessage}
          </Alert>
        )}
        {!name &&
          recordings &&
          (recordings.recordings.length ? (
            <div className="recording-list">
              {recordings.recordings.map((entry) => (
                <a
                  href={`recording.html?name=${encodeURIComponent(entry.name)}`}
                  key={entry.name}
                >
                  <strong>{recordingTitle(entry.name)}</strong>
                  <span>
                    {(entry.sizeBytes / 1_000_000).toFixed(1)} MB ·{" "}
                    {entry.mcapPath ? "recording data" : "no recording data"}
                    {recordings.active === entry.name ? " · recording now" : ""}
                  </span>
                </a>
              ))}
            </div>
          ) : (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>No recordings yet.</EmptyTitle>
              </EmptyHeader>
            </Empty>
          ))}
        {loaded && (
          <Playback
            recording={loaded}
            preferences={preferences}
            colorScheme={colorScheme}
          />
        )}
      </main>
    </ViewerShell>
  );
}
