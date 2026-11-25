import type { SyncedClip } from "@hebbian/mcap-tools/playback-clock";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button.js";
import type { RecordingClip } from "../../recording-timeline.js";

export function PlaybackVideo({
  clips,
  loadVideo,
  register,
}: {
  clips: readonly RecordingClip[];
  loadVideo: (clip: RecordingClip) => Promise<Uint8Array>;
  register: (segment: string, clip: SyncedClip | null) => void;
}) {
  const [activeSegments, setActiveSegments] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [focused, setFocused] = useState(false);
  const onActive = useCallback((segment: string, active: boolean) => {
    setActiveSegments((previous) => {
      if (previous.has(segment) === active) return previous;
      const next = new Set(previous);
      if (active) next.add(segment);
      else next.delete(segment);
      return next;
    });
  }, []);
  return (
    <figure className={`playback-video ${focused ? "is-focused" : ""}`}>
      <figcaption>{clips[0]?.cameraName.replaceAll("_", " ")}</figcaption>
      <div className="playback-picture">
        {clips.map((clip) => (
          <PlaybackSegment
            key={clip.segment}
            clip={clip}
            loadVideo={loadVideo}
            register={register}
            onActive={onActive}
          />
        ))}
        {activeSegments.size === 0 && <p>Not recording at this time</p>}
        <Button variant="outline" onClick={() => setFocused((value) => !value)}>
          {focused ? "Restore size" : "Enlarge"}
        </Button>
      </div>
    </figure>
  );
}

function PlaybackSegment({
  clip,
  loadVideo,
  register,
  onActive,
}: {
  clip: RecordingClip;
  loadVideo: (clip: RecordingClip) => Promise<Uint8Array>;
  register: (segment: string, clip: SyncedClip | null) => void;
  onActive: (segment: string, active: boolean) => void;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const [idle, setIdle] = useState(true);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const failedClip = useRef(false);
  useEffect(() => {
    const element = video.current;
    if (!element) return;
    let idleState = true;
    let objectUrl: string | null = null;
    let cancelled = false;
    // CONSTRAINT: The clip's video is packaged from the recording's MCAP, so it is local
    // once it has loaded and `cacheLocally` has nothing more to fetch.
    const loaded = loadVideo(clip)
      .then((bytes) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(
          new Blob([new Uint8Array(bytes)], { type: "video/mp4" }),
        );
        element.src = objectUrl;
      })
      .catch(() => {
        if (cancelled) return;
        failedClip.current = true;
        setFailed(true);
      });
    const synced: SyncedClip = {
      video: element,
      startTime: clip.startTime,
      durationSeconds: clip.durationSeconds,
      get failed() {
        return failedClip.current;
      },
      setIdle(next) {
        if (next !== idleState) {
          idleState = next;
          setIdle(next);
          onActive(clip.segment, !next);
        }
      },
      cacheLocally: () => loaded,
    };
    register(clip.segment, synced);
    return () => {
      cancelled = true;
      register(clip.segment, null);
      onActive(clip.segment, false);
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [clip, loadVideo, register, onActive]);
  return (
    <div className="playback-segment" hidden={idle}>
      <video
        ref={video}
        muted
        playsInline
        preload="auto"
        onLoadedData={() => setLoading(false)}
        onError={() => {
          failedClip.current = true;
          setFailed(true);
        }}
        aria-label={`${clip.cameraName} recording`}
      />
      {(loading || failed) && (
        <p>{failed ? "Could not load this clip" : "Loading clip…"}</p>
      )}
    </div>
  );
}
