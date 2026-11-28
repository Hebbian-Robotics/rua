import { Effect } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  type HttpClientResponse,
  type HttpMethod,
} from "effect/http";
import { z } from "zod";

import { StationHttpError, StationTimeoutError } from "./station-errors.js";

const errorBodySchema = z.object({ detail: z.string() });

/** How long an HTTP request to the station may take. */
const HTTP_TIMEOUT_MILLISECONDS = 30_000;
const MAX_PLAIN_TEXT_ERROR_LENGTH = 500;

/**
 * Provide the fetch-backed HTTP client, resolving `globalThis.fetch` each time the
 * effect runs.
 *
 * WHY: `FetchHttpClient.Fetch` defaults to the `fetch` seen on first use and caches
 * it, so a replaced global `fetch` would otherwise never be used.
 */
export function provideStationHttpClient<Result, Failure, Requirements>(
  effect: Effect.Effect<Result, Failure, Requirements>,
): Effect.Effect<
  Result,
  Failure,
  Exclude<Requirements, HttpClient.HttpClient>
> {
  return Effect.suspend(() =>
    effect.pipe(
      Effect.provide(FetchHttpClient.layer),
      Effect.provideService(FetchHttpClient.Fetch, globalThis.fetch),
    ),
  );
}

function parseJsonOrNull(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function stationHttpError(
  status: number,
  body: string,
  contentType: string | undefined,
): StationHttpError {
  const detail = errorBodySchema.safeParse(parseJsonOrNull(body));
  const plainText =
    contentType?.split(";")[0] === "text/plain"
      ? body.trim().slice(0, MAX_PLAIN_TEXT_ERROR_LENGTH)
      : "";
  return new StationHttpError(
    status,
    body,
    detail.success ? detail.data.detail : plainText || `HTTP ${status}`,
  );
}

function stationResponseEffect<Result>(
  url: URL,
  method: HttpMethod.HttpMethod,
  body: unknown,
  consume: (
    response: HttpClientResponse.HttpClientResponse,
  ) => Effect.Effect<Result, unknown>,
): Effect.Effect<Result, unknown, HttpClient.HttpClient> {
  return Effect.gen(function* () {
    const client = HttpClient.withScope(yield* HttpClient.HttpClient);
    const request = yield* Effect.try({
      try: () => {
        const initial = HttpClientRequest.make(method)(url);
        return body === undefined
          ? initial
          : HttpClientRequest.bodyText(
              initial,
              JSON.stringify(body),
              "application/json",
            );
      },
      catch: (error) => error,
    });
    const response = yield* client.execute(request);
    if (response.status < 200 || response.status >= 300) {
      const errorBody = yield* response.text.pipe(
        Effect.orElseSucceed(() => ""),
      );
      return yield* Effect.fail(
        stationHttpError(
          response.status,
          errorBody,
          response.headers["content-type"],
        ),
      );
    }
    return yield* consume(response);
  }).pipe(
    Effect.scoped,
    Effect.timeoutOrElse({
      duration: HTTP_TIMEOUT_MILLISECONDS,
      orElse: () =>
        Effect.fail(new StationTimeoutError("Station request timed out")),
    }),
    Effect.provideService(HttpClient.TracerPropagationEnabled, false),
    Effect.mapError((error) =>
      HttpClientError.isHttpClientError(error) &&
      (error.reason._tag === "TransportError" ||
        error.reason._tag === "DecodeError") &&
      error.reason.cause !== undefined
        ? error.reason.cause
        : error,
    ),
  );
}

/** Fetch and decode a JSON response within the current Effect HTTP scope. */
export function stationJsonEffect(
  url: URL,
  method: HttpMethod.HttpMethod = "GET",
  body?: unknown,
): Effect.Effect<unknown, unknown, HttpClient.HttpClient> {
  return stationResponseEffect(url, method, body, (response) => response.json);
}

/** Decode a JSON route response at the HTTP boundary. */
export function stationParsedJsonEffect<Result>(
  url: URL,
  method: HttpMethod.HttpMethod,
  parse: (payload: unknown) => Result,
  body?: unknown,
): Effect.Effect<Result, unknown, HttpClient.HttpClient> {
  return stationJsonEffect(url, method, body).pipe(
    Effect.flatMap((payload) =>
      Effect.try({
        try: () => parse(payload),
        catch: (error) => error,
      }),
    ),
  );
}

/** Read a binary response before releasing its HTTP scope. */
export function stationBinaryEffect(
  url: URL,
  method: HttpMethod.HttpMethod,
  body?: unknown,
) {
  return stationResponseEffect(url, method, body, (response) =>
    response.arrayBuffer.pipe(
      Effect.map((buffer) => ({
        bytes: new Uint8Array(buffer),
        headers: response.headers,
      })),
    ),
  );
}

/** Send a request whose successful response has no body. */
export function stationVoidEffect(
  url: URL,
  method: HttpMethod.HttpMethod,
  body?: unknown,
) {
  return stationResponseEffect(url, method, body, () => Effect.void);
}

/** Execute a generated route using the ordinary Promise interface. */
export async function runStationEffect<Result>(
  operation: Effect.Effect<Result, unknown, HttpClient.HttpClient>,
  signal?: AbortSignal,
): Promise<Result> {
  signal?.throwIfAborted();
  try {
    const result = await Effect.runPromise(
      operation.pipe(
        provideStationHttpClient,
        Effect.match({
          onFailure: (error) => ({ kind: "failure" as const, error }),
          onSuccess: (value) => ({ kind: "success" as const, value }),
        }),
      ),
      { signal },
    );
    if (result.kind === "failure") throw result.error;
    return result.value;
  } catch (error) {
    signal?.throwIfAborted();
    throw error;
  }
}

/** Fetch from the station, throwing `StationHttpError` for an error status. */
export async function stationFetch(
  url: string | URL,
  init?: RequestInit,
): Promise<Response> {
  const deadline = AbortSignal.timeout(HTTP_TIMEOUT_MILLISECONDS);
  const response = await fetch(url, {
    ...init,
    signal: init?.signal ? AbortSignal.any([deadline, init.signal]) : deadline,
  });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw stationHttpError(
      response.status,
      body,
      response.headers.get("content-type") ?? undefined,
    );
  }
  return response;
}

/**
 * Fetch JSON from the station; a failed request throws a `StationHttpError`
 * with the station's JSON detail or plain-text explanation when supplied.
 */
export async function stationRequest(
  url: string | URL,
  init?: RequestInit,
): Promise<unknown> {
  return (await stationFetch(url, init)).json();
}

/** RequestInit for a JSON body, or none when there is no body. */
export function jsonRequestInit(
  method: string,
  body?: unknown,
  signal?: AbortSignal,
): RequestInit {
  return body === undefined
    ? { method, signal }
    : {
        method,
        signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      };
}
