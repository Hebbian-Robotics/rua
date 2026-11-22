import { apiSchemas } from "hebbian-rua";
import {
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
  feeds: CameraFeed[];
  roomMetadata: string | undefined;
};

export function useCameraRoom(): CameraRoomState {
  const [state, setState] = useState<CameraRoomState>({
    status: "Connecting to cameras…",
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
        status: orderedFeeds.length ? "" : "No cameras streaming.",
      }));
    }

    async function connect(): Promise<void> {
      const connecting = new Room({ adaptiveStream: true });
      room = connecting;
      connecting
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
            status: "Camera connection lost; reconnecting…",
            feeds: [],
            roomMetadata: undefined,
          });
          cameraActor.send({ type: "DISCONNECTED" });
        });
      } catch (error: unknown) {
        connecting.removeAllListeners();
        await connecting.disconnect();
        if (!active) return;
        setState({
          status: `Cameras unavailable: ${errorMessage(error, "unknown error")}`,
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
  return state;
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
