// CONSTRAINT: Vitest reads the environment directive to provide browser globals.
// @vitest-environment jsdom

import type { SyncedClip } from "@hebbian/mcap-tools/playback-clock";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { PlaybackVideo } from "../src/features/recordings/playback-video.js";
import type { RecordingClip } from "../src/recording-timeline.js";

test("reloading a failed segment replaces the old error with loading", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  const root = createRoot(container);
  const clips: RecordingClip[] = [
    {
      cameraName: "wrist",
      segment: "segment-1",
      startTime: 1,
      durationSeconds: 1,
      startNanoseconds: 1_000_000_000n,
      endNanoseconds: 2_000_000_000n,
      width: 16,
      height: 8,
    },
  ];
  function register(_segment: string, synced: SyncedClip | null) {
    synced?.setIdle(false);
  }
  const visibleMessages = () =>
    [...container.querySelectorAll("p")]
      .filter((message) => !message.closest("[hidden]"))
      .map((message) => message.textContent);
  try {
    await act(async () => {
      root.render(
        createElement(PlaybackVideo, {
          clips,
          loadVideo: () => Promise.reject(new Error("Could not decode clip")),
          register,
        }),
      );
    });
    expect(visibleMessages()).toEqual(["Could not load this clip"]);

    const nextClip = new Promise<Uint8Array>(() => undefined);
    await act(async () => {
      root.render(
        createElement(PlaybackVideo, {
          clips,
          loadVideo: () => nextClip,
          register,
        }),
      );
    });
    expect(visibleMessages()).toEqual(["Loading clip…"]);
    expect(container.textContent).not.toContain("Could not load this clip");
  } finally {
    await act(async () => root.unmount());
    vi.unstubAllGlobals();
  }
});
