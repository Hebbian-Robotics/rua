// Whether the station is recording, from the LiveKit room's metadata, which the
// station sets when a recording starts and stops (the `runs` crate announces it).
import type { Room } from "livekit-client";

export function roomIsRecording(room: Room): boolean {
  try {
    const metadata: unknown = JSON.parse(room.metadata ?? "");
    return (
      typeof metadata === "object" &&
      metadata !== null &&
      "recording" in metadata &&
      metadata.recording === true
    );
  } catch {
    return false;
  }
}
