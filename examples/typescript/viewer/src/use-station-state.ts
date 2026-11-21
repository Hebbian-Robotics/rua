import { useEffect, useSyncExternalStore } from "react";
import { stationConnection } from "./viewer-connection.js";

export function useStationState() {
  const stationState = useSyncExternalStore(
    stationConnection.onState,
    stationConnection.getState,
  );
  const connected = useSyncExternalStore(
    stationConnection.onConnectionChange,
    stationConnection.isConnected,
  );

  useEffect(() => {
    stationConnection.start();
  }, []);

  return { stationState, connected };
}
