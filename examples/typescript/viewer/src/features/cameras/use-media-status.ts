import { useQuery } from "@tanstack/react-query";
import {
  type MediaStatus,
  mediaStatusSchema,
} from "../../../shared/media-status-schema.js";
import { stationJson } from "../../api.js";

export function useMediaStatus(): string {
  const query = useQuery<MediaStatus>({
    queryKey: ["media-status"],
    queryFn: async () =>
      mediaStatusSchema.parse(await stationJson("api/media-status")),
    refetchInterval: 5000,
    refetchIntervalInBackground: true,
    retry: false,
  });
  const details = query.data ? mediaStatusDetails(query.data) : "";
  if (!query.isError) return details;
  const unavailable = "Camera status could not be refreshed";
  return details ? `${unavailable} · ${details}` : unavailable;
}

function mediaStatusDetails({ service, cameras }: MediaStatus): string {
  switch (service.state) {
    case "running":
      return Object.entries(cameras)
        .flatMap(([name, camera]) => {
          switch (camera.state) {
            case "resolving":
            case "starting":
              return [];
            case "disconnected":
              return [`${name}: camera disconnected`];
            case "ambiguous":
              return [`${name}: multiple USB cameras matched`];
            case "stalled":
              return [`${name}: capture stalled, retrying`];
            case "failed":
              return [`${name}: ${camera.detail}`];
            case "capturing":
              return camera.publication.state === "reconnecting"
                ? [`${name}: local capture active, reconnecting to LiveKit`]
                : [];
            default: {
              const unexpected: never = camera;
              return unexpected;
            }
          }
        })
        .join(" · ");
    case "restarting":
      return `Media service restarting: ${service.last_exit}`;
    case "unavailable":
      return `Media service unavailable: ${service.reason}`;
  }
}
