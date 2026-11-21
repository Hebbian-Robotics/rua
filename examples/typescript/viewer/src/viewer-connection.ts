import { createStationConnection } from "@hebbian/robot-use-client";
export const stationConnection = createStationConnection(
  () => globalThis.location.href,
  "manual",
);
