import { apiSchemas, StationHttpError } from "hebbian-rua";
import {
  ConnectionState,
  type RemoteParticipant,
  type RemoteTrack,
  Room,
  RoomEvent,
  Track,
} from "livekit-client";
import { TriangleAlertIcon } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import { createActor } from "xstate";
import { Badge } from "@/components/ui/badge.js";
import { Button } from "@/components/ui/button.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";
import { createCameraConnectionMachine } from "./camera-connection-machine.js";

type CameraFeed = { name: string } & (
  | { kind: "room"; track: RemoteTrack }
  | { kind: "local" }
);
export type CameraRoomState = {
  status: string;
  localCamerasOnly: boolean;
  feeds: CameraFeed[];
  roomMetadata: string | undefined;
};

type CameraConnection =
  | "local"
  | "connecting"
  | "connected"
  | "reconnecting"
  | { error: string };

function connectionMessage(
  connection: CameraConnection,
  hasFeeds: boolean,
): string {
  switch (connection) {
    case "local":
      return hasFeeds
        ? "Showing local cameras. Remote cameras are disabled."
        : "Local cameras only. Waiting for a connected camera.";
    case "connecting":
      return "Connecting to LiveKit…";
    case "connected":
      return hasFeeds
        ? "LiveKit connected."
        : "LiveKit connected. Waiting for cameras.";
    case "reconnecting":
      return "Reconnecting to LiveKit…";
    default:
      return `Couldn’t connect to LiveKit. Retrying. ${connection.error}`;
  }
}

export function useCameraRoom(): CameraRoomState {
  const [state, setState] = useState<
    Omit<CameraRoomState, "status" | "localCamerasOnly"> & {
      connection: CameraConnection;
    }
  >({
    connection: "connecting",
    feeds: [],
    roomMetadata: undefined,
  });

  useEffect(() => {
    let active = true;
    let room: Room | null = null;
    const feeds = new Map<string, RemoteTrack>();

    function updateFeeds(): void {
      if (!active) return;
      const orderedFeeds = [...feeds.entries()]
        .sort(
          ([first], [second]) =>
            Number(first.startsWith("phone_")) -
              Number(second.startsWith("phone_")) ||
            first.localeCompare(second),
        )
        .map(([name, track]) => ({ name, kind: "room" as const, track }));
      setState((previous) => ({
        ...previous,
        feeds: orderedFeeds,
      }));
    }

    async function connect(): Promise<void> {
      const connecting = new Room({ adaptiveStream: true });
      room = connecting;
      connecting
        .on(RoomEvent.ConnectionStateChanged, (connectionState) => {
          if (!active) return;
          if (connectionState === ConnectionState.Connected) {
            setState((previous) => ({ ...previous, connection: "connected" }));
          } else if (
            connectionState === ConnectionState.Reconnecting ||
            connectionState === ConnectionState.SignalReconnecting
          ) {
            setState((previous) => ({
              ...previous,
              connection: "reconnecting",
            }));
          }
        })
        .on(
          RoomEvent.TrackSubscribed,
          (
            track: RemoteTrack,
            _publication: unknown,
            participant: RemoteParticipant,
          ) => {
            if (!active || track.kind !== Track.Kind.Video) return;
            feeds.set(participant.identity, track);
            updateFeeds();
          },
        )
        .on(
          RoomEvent.TrackUnsubscribed,
          (
            track: RemoteTrack,
            _publication: unknown,
            participant: RemoteParticipant,
          ) => {
            if (feeds.get(participant.identity) !== track) return;
            feeds.delete(participant.identity);
            updateFeeds();
          },
        )
        .on(RoomEvent.RoomMetadataChanged, () => {
          if (active)
            setState((previous) => ({
              ...previous,
              roomMetadata: connecting.metadata,
            }));
        });
      try {
        const access = apiSchemas.shape.RoomAccess.parse(
          await stationJson("/api/media/access-token"),
        );
        if (!active) return;
        await connecting.connect(access.server_url, access.token);
        if (!active) {
          await connecting.disconnect();
          return;
        }
        cameraActor.send({ type: "CONNECTED" });
        updateFeeds();
        setState((previous) => ({
          ...previous,
          roomMetadata: connecting.metadata,
        }));
        connecting.on(RoomEvent.Disconnected, () => {
          if (!active) return;
          connecting.removeAllListeners();
          feeds.clear();
          setState({
            connection: "reconnecting",
            feeds: [],
            roomMetadata: undefined,
          });
          cameraActor.send({ type: "DISCONNECTED" });
        });
      } catch (error: unknown) {
        connecting.removeAllListeners();
        await connecting.disconnect();
        if (!active) return;
        feeds.clear();
        if (error instanceof StationHttpError && error.status === 409) {
          setState((previous) => ({
            ...previous,
            feeds: [],
            connection: "local",
          }));
          cameraActor.stop();
          return;
        }
        setState({
          connection: { error: errorMessage(error, "Unknown error.") },
          feeds: [],
          roomMetadata: undefined,
        });
        cameraActor.send({ type: "FAILED" });
      }
    }

    const cameraMachine = createCameraConnectionMachine(connect);
    const cameraActor = createActor(cameraMachine);
    cameraActor.start();
    return () => {
      active = false;
      cameraActor.stop();
      room?.removeAllListeners();
      void room?.disconnect();
    };
  }, []);
  const { connection, ...roomState } = state;
  const localCamerasOnly = connection === "local";
  const localCameras = useQuery({
    queryKey: ["local-cameras"],
    queryFn: async ({ signal }) =>
      apiSchemas.shape.CameraListing.array().parse(
        await stationJson("/api/media/local-cameras", { signal }),
      ),
    enabled: localCamerasOnly,
    refetchInterval: 2000,
  });
  const feeds: CameraFeed[] = localCamerasOnly
    ? (localCameras.data ?? [])
        .filter(({ available }) => available)
        .map(({ name }) => ({ name, kind: "local" }))
    : roomState.feeds;
  return {
    ...roomState,
    feeds,
    localCamerasOnly,
    status:
      localCamerasOnly && localCameras.isError
        ? "Local camera service unavailable."
        : connectionMessage(connection, feeds.length > 0),
  };
}

function cameraTitle(name: string): string {
  return name.startsWith("phone_")
    ? `Phone · ${name.slice(6).replaceAll("_", " ")}`
    : name.replaceAll("_", " ");
}

const LOCAL_FRAME_INTERVAL_MILLISECONDS = 125;
const LOCAL_FRAME_RETRY_MILLISECONDS = 1000;

function waitOrAbort(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const stopWaiting = () => {
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", stopWaiting);
      resolve();
    }, milliseconds);
    signal.addEventListener("abort", stopWaiting, { once: true });
  });
}

// WHY: Short snapshot requests leave browser connections free for controls, and a failed
// fetch simply retries, so a preview recovers on its own after the camera comes back.
function useLatestLocalCameraFrame(
  cameraName: string | undefined,
): string | undefined {
  const [frameUrl, setFrameUrl] = useState<string>();
  useEffect(() => {
    if (cameraName === undefined) return;
    const stopped = new AbortController();
    let shownFrameUrl: string | undefined;
    void (async () => {
      while (!stopped.signal.aborted) {
        let delay = LOCAL_FRAME_INTERVAL_MILLISECONDS;
        try {
          const response = await fetch(
            `/api/cameras/${encodeURIComponent(cameraName)}`,
            { cache: "no-store", signal: stopped.signal },
          );
          if (!response.ok) throw new Error(`HTTP ${response.status}`);
          const nextFrameUrl = URL.createObjectURL(await response.blob());
          if (shownFrameUrl) URL.revokeObjectURL(shownFrameUrl);
          shownFrameUrl = nextFrameUrl;
          setFrameUrl(nextFrameUrl);
        } catch {
          delay = LOCAL_FRAME_RETRY_MILLISECONDS;
        }
        await waitOrAbort(delay, stopped.signal);
      }
    })();
    return () => {
      stopped.abort();
      if (shownFrameUrl) URL.revokeObjectURL(shownFrameUrl);
      setFrameUrl(undefined);
    };
  }, [cameraName]);
  return frameUrl;
}

export const CameraView = memo(function CameraView({
  feed,
  showLabel,
}: {
  feed: CameraFeed;
  showLabel: boolean;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const container = useRef<HTMLElement>(null);
  const track = feed.kind === "room" ? feed.track : undefined;
  const localFrameUrl = useLatestLocalCameraFrame(
    feed.kind === "local" ? feed.name : undefined,
  );
  useEffect(() => {
    const element = video.current;
    if (!element || !track) return;
    track.attach(element);
    return () => {
      track.detach(element);
    };
  }, [track]);
  const label = `${cameraTitle(feed.name)} camera feed`;
  return (
    <article className="camera-view" ref={container}>
      {showLabel && (
        <header>
          <h3>{cameraTitle(feed.name)}</h3>
        </header>
      )}
      <div className="camera-picture">
        {feed.kind === "room" ? (
          <video ref={video} autoPlay muted playsInline aria-label={label} />
        ) : (
          <>
            {localFrameUrl && (
              <img src={localFrameUrl} alt={label} width={640} height={360} />
            )}
            <Badge
              variant="destructive"
              className="camera-fallback-label"
              title="Local fallback preview. Prefer LiveKit whenever it is available."
            >
              <TriangleAlertIcon aria-hidden="true" />
              Fallback · LiveKit unavailable
            </Badge>
          </>
        )}
        <Button
          variant="outline"
          onClick={() => {
            if (document.fullscreenElement) void document.exitFullscreen();
            else void container.current?.requestFullscreen();
          }}
        >
          Fullscreen
        </Button>
      </div>
    </article>
  );
});

import { useQuery } from "@tanstack/react-query";
