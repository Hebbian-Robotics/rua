import { apiSchemas, StationHttpError } from "hebbian-rua";
import {
  ConnectionState,
  type RemoteParticipant,
  type RemoteTrack,
  Room,
  RoomEvent,
  Track,
} from "livekit-client";
import { memo, useEffect, useRef, useState } from "react";
import { createActor } from "xstate";
import { Button } from "@/components/ui/button.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";
import { createCameraConnectionMachine } from "./camera-connection-machine.js";

export type CameraFeed = { name: string; track: RemoteTrack };
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
      return "USB cameras only. Local snapshots are available from the station.";
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
        .map(([name, track]) => ({ name, track }));
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
          await stationJson("api/token"),
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
  return {
    ...roomState,
    localCamerasOnly: connection === "local",
    status: connectionMessage(connection, state.feeds.length > 0),
  };
}

function cameraTitle(name: string): string {
  return name.startsWith("phone_")
    ? `Phone · ${name.slice(6).replaceAll("_", " ")}`
    : name.replaceAll("_", " ");
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
  useEffect(() => {
    const element = video.current;
    if (!element) return;
    feed.track.attach(element);
    return () => {
      feed.track.detach(element);
    };
  }, [feed.track]);
  return (
    <article className="camera-view" ref={container}>
      {showLabel && (
        <header>
          <h3>{cameraTitle(feed.name)}</h3>
        </header>
      )}
      <div className="camera-picture">
        <video
          ref={video}
          autoPlay
          muted
          playsInline
          aria-label={`${cameraTitle(feed.name)} camera feed`}
        />
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
