// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import { useMediaStatus } from "../src/features/cameras/use-media-status.js";

function CameraStatusProbe() {
  return createElement("p", { role: "status" }, useMediaStatus());
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

test("a failed refresh preserves the last camera warning until a healthy response", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const request = vi
    .fn()
    .mockResolvedValueOnce(
      Response.json(
        { detail: "Camera status could not be refreshed" },
        { status: 503 },
      ),
    )
    .mockResolvedValueOnce(
      Response.json({
        service: { state: "running" },
        cameras: {
          wrist: { state: "capturing", publication: { state: "publishing" } },
        },
      }),
    );
  vi.stubGlobal("fetch", request);
  const client = new QueryClient({
    defaultOptions: { queries: { gcTime: Infinity, staleTime: Infinity } },
  });
  client.setQueryData(["media-status"], {
    service: { state: "running" },
    cameras: { wrist: { state: "ambiguous" } },
  });
  const view = document.createElement("div");
  const root = createRoot(view);
  async function refresh() {
    await act(async () => {
      await client.refetchQueries({ queryKey: ["media-status"] });
      await vi.advanceTimersByTimeAsync(0);
    });
  }

  try {
    await act(async () => {
      root.render(
        createElement(
          QueryClientProvider,
          { client },
          createElement(CameraStatusProbe),
        ),
      );
    });
    expect(view.textContent).toBe("wrist: multiple USB cameras matched");

    await refresh();
    expect(view.textContent).toContain("Camera status could not be refreshed");
    expect(view.textContent).toContain("wrist: multiple USB cameras matched");

    await refresh();
    expect(view.textContent).toBe("");
  } finally {
    await act(async () => root.unmount());
    client.clear();
  }
});
