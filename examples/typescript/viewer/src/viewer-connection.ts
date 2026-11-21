import { createStationConnection } from "hebbian-rua";
export const stationConnection = createStationConnection(
  () => globalThis.location.href,
  "manual",
);
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => stationConnection.close());
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) stationConnection.start();
  });
}
