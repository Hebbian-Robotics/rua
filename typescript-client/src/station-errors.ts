// The failures a caller branches on, matching the Python and Rust clients.

import type { LeaseAnswer } from "./station-types.js";

/**
 * The station could not be reached, the connection was lost before an answer
 * arrived, or the station sent something unreadable. A command that failed this
 * way may or may not have been accepted; the client never retries it.
 */
export class StationConnectionError extends Error {
  override name = "StationConnectionError";
}

/** The station did not answer in time. */
export class StationTimeoutError extends Error {
  override name = "StationTimeoutError";
}

/** The station refused a message it could not act on; nothing moved. */
export class StationRefusedError extends Error {
  override name = "StationRefusedError";
  constructor(readonly detail: string) {
    super(detail);
  }
}

/** Someone else holds the lease, and it was acquired rather than taken. */
export class LeaseHeldElsewhereError extends Error {
  override name = "LeaseHeldElsewhereError";
  constructor(readonly answer: Exclude<LeaseAnswer, { kind: "granted" }>) {
    super(
      answer.status === null
        ? "The lease is held by someone else"
        : `The lease is held by ${answer.status.holder} '${answer.status.label}'`,
    );
  }
}

/** An HTTP route answered with an error status. */
export class StationHttpError extends Error {
  override name = "StationHttpError";
  constructor(
    readonly status: number,
    readonly body: string,
    message: string,
  ) {
    super(message);
  }
}

/**
 * The media service has no fresh frame for this camera. `listed` is what it reports: a
 * camera missing from it was never published or the name is wrong, while one listed as
 * unavailable stopped streaming. `lastError` is the latest failure the station saw in the
 * media service's output for it, when the station could be asked.
 */
export class CameraUnavailableError extends Error {
  override name = "CameraUnavailableError";
  constructor(
    readonly camera: string,
    readonly listed: ReadonlyArray<{ name: string; available: boolean }>,
    readonly lastError: string | null = null,
  ) {
    super(
      `camera '${camera}' is not streaming (${
        listed.length === 0
          ? "the media service lists no cameras; check that it is running and the station's --camera device ids are current"
          : `the media service lists: ${listed
              .map(
                ({ name, available }) =>
                  `${name} (${available ? "streaming" : "unavailable"})`,
              )
              .join(", ")}`
      })${lastError === null ? "" : `; last error: ${lastError}`}`,
    );
  }
}

/** The arm faulted or was disabled before it recovered. */
export class ArmRecoveryEndedError extends Error {
  override name = "ArmRecoveryEndedError";
}

/** This client no longer holds the lease: taken, revoked, returned, lapsed or disconnected. */
export class LeaseLostError extends Error {
  override name = "LeaseLostError";
  constructor() {
    super("The lease was lost");
  }
}
