import { afterEach, expect, test, vi } from "vitest";

import { createStationHttpClient } from "../src/index.js";

afterEach(() => vi.unstubAllGlobals());

function stubFetch(answer: (url: URL) => Response) {
  const requested: Array<{ url: string; method: string; body: unknown }> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: URL, init?: RequestInit) => {
      requested.push({
        url: String(url),
        method: init?.method ?? "GET",
        body: init?.body,
      });
      return answer(url);
    }),
  );
  return requested;
}

test("cameras list only the available ones and snapshots carry the capture time", async () => {
  const requested = stubFetch((url) =>
    url.pathname === "/cameras"
      ? Response.json([
          { name: "wrist", available: true },
          { name: "gone", available: false },
        ])
      : new Response(new Uint8Array([1, 2, 3]), {
          headers: { "X-Captured-At-Unix-Ns": "1700000000000000001" },
        }),
  );
  const client = createStationHttpClient();
  expect(await client.cameraNames()).toEqual(["wrist"]);
  const frame = await client.cameraJpeg("wrist");
  expect([...frame.jpeg]).toEqual([1, 2, 3]);
  expect(frame.capturedAtUnixNanoseconds).toBe(1700000000000000001n);
  expect(requested.map(({ url }) => url)).toEqual([
    "http://127.0.0.1:8781/cameras",
    "http://127.0.0.1:8781/cameras/wrist",
  ]);
});

test("a camera without a frame names the cameras the media service lists", async () => {
  stubFetch((url) => {
    if (url.pathname === "/cameras")
      return Response.json([{ name: "wrist", available: false }]);
    if (url.pathname === "/viewer/api/media-status")
      return Response.json({
        programs: {},
        cameras: {
          wrist: {
            detail: "camera backend: Device is null",
            last_seen_unix_seconds: 1,
          },
        },
      });
    return new Response("", { status: 404 });
  });
  const failure = await createStationHttpClient()
    .cameraJpeg("wrist")
    .catch((error: unknown) => error);
  expect(failure).toMatchObject({
    name: "CameraUnavailableError",
    camera: "wrist",
    listed: [{ name: "wrist", available: false }],
    lastError: "camera backend: Device is null",
  });
  expect(String(failure)).toContain("wrist (unavailable)");
  expect(String(failure)).toContain(
    "last error: camera backend: Device is null",
  );
});

test("phone invites post to the station and traces are listed and downloaded from it", async () => {
  const requested = stubFetch((url) => {
    if (url.pathname === "/viewer/api/phone-invites")
      return Response.json({
        camera_name: "phone_table",
        url: "https://x",
        qr_code_image: "data:",
      });
    if (url.pathname === "/traces")
      return Response.json([
        { name: "a.gz", sizeBytes: 5, modifiedUnixSeconds: 1.5 },
      ]);
    return new Response(new Uint8Array([31, 139]));
  });
  const client = createStationHttpClient({ stationUrl: "http://station:1" });
  expect((await client.phoneInvite("table")).camera_name).toBe("phone_table");
  expect(requested[0]).toMatchObject({
    method: "POST",
    body: JSON.stringify({ label: "table" }),
  });
  expect(await client.listTraces()).toEqual([
    { name: "a.gz", sizeBytes: 5, modifiedUnixSeconds: 1.5 },
  ]);
  expect([...(await client.downloadTrace("a.gz"))]).toEqual([31, 139]);
});

test("an error status is a StationHttpError carrying the station's detail", async () => {
  stubFetch(() => Response.json({ detail: "no such trace" }, { status: 404 }));
  const failure = await createStationHttpClient()
    .downloadTrace("x")
    .catch((error: unknown) => error);
  expect(failure).toMatchObject({
    name: "StationHttpError",
    status: 404,
    message: "no such trace",
  });
});
