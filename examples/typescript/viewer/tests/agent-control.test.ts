// @vitest-environment jsdom

import type { StationState } from "hebbian-rua";
import { act, type ComponentProps, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { AgentControl } from "../src/features/agent/agent-control.js";
import { readViewerPreferences } from "../src/features/settings/preferences.js";
import { useViewerPreferences } from "../src/features/settings/use-viewer-preferences.js";
import type { RunRequest } from "../src/run-logic.js";

const stationState: StationState = {
  arms: [
    {
      active_command: null,
      active_trip: null,
      arm: "follower",
      base_pose: null,
      commanded_joints: [0],
      controller: "ik",
      fault: null,
      gripper_opening: null,
      interlocks: [],
      last_command: null,
      measured_joint_efforts: [0],
      measured_joint_velocities: [0],
      measured_joints: [0],
      mode: "holding",
      recovery_issue: null,
      rest_pose: [0],
      revision: 0,
      sampled_unix_nano: "1",
      segment: null,
      tool: null,
    },
  ],
  description: null,
  jaw_closing_axis: "x",
  joint_names: ["joint"],
  lease: null,
  max_joint_speed: 1,
  robot: null,
  surface_guard: null,
};

const agent: ComponentProps<typeof AgentControl>["agent"] = {
  run: null,
  setRun: () => {},
  loadLessons: async () => [],
  saveFeedback: async () => {
    throw new Error("No run to save feedback for");
  },
  status: "reachable",
  providers: [
    {
      provider: "openai",
      label: "OpenAI",
      available: true,
      models: [{ id: "gpt-6-sol", label: "GPT-6 Sol" }],
    },
    {
      provider: "gemini",
      label: "Gemini",
      available: true,
      models: [{ id: "gemini-4-hydrogen", label: "Gemini 4 Hydrogen" }],
    },
  ],
  connectionError: "",
  providersError: "",
};

function AgentControlWithPreferences({
  onStartRun,
  agentState = agent,
}: Pick<ComponentProps<typeof AgentControl>, "onStartRun"> & {
  agentState?: typeof agent;
}) {
  const { preferences, updatePreferences } = useViewerPreferences();
  return createElement(AgentControl, {
    stationState,
    cameraNames: [],
    agent: agentState,
    startingRun: false,
    onStartRun,
    runError: "",
    armName: "follower",
    modelSelection: preferences.agentModelSelection,
    onModelSelectionChange: (agentModelSelection) =>
      updatePreferences({ ...preferences, agentModelSelection }),
    onStopRun: async () => {},
    stoppingRun: false,
  });
}

let view: HTMLDivElement;
let root: Root | undefined;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("matchMedia", () => ({
    matches: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  view = document.createElement("div");
  document.body.append(view);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = undefined;
  view.remove();
  document.documentElement.removeAttribute("data-theme");
  localStorage.clear();
  vi.unstubAllGlobals();
});

async function mountAgentControl(
  requests: RunRequest[],
  agentState: typeof agent = agent,
) {
  await act(async () => {
    root?.unmount();
    root = createRoot(view);
    root.render(
      createElement(AgentControlWithPreferences, {
        onStartRun: async (request) => {
          requests.push(request);
        },
        agentState,
      }),
    );
  });
}

function requireControl<Control extends HTMLElement>(
  selector: string,
): Control {
  const control = view.querySelector<Control>(selector);
  if (!control) throw new Error(`Missing form control: ${selector}`);
  return control;
}

function providerSelect() {
  return requireControl<HTMLSelectElement>('select[name="provider"]');
}

function modelSelect() {
  return requireControl<HTMLSelectElement>('select[name="model"]');
}

function startButton() {
  return requireControl<HTMLButtonElement>('button[type="submit"]');
}

function customModelInput() {
  return requireControl<HTMLInputElement>('input[name="customModel"]');
}

async function selectOption(select: HTMLSelectElement, label: string) {
  const option = Array.from(select.options).find(
    (option) => option.label === label,
  );
  if (!option) throw new Error(`Missing option: ${label}`);
  await act(async () => {
    select.value = option.value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

async function enterText(
  field: HTMLInputElement | HTMLTextAreaElement,
  value: string,
) {
  await act(async () => {
    field.setRangeText(value, 0, field.value.length);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

test("a custom model is saved, restored on reload, and submitted by ID", async () => {
  const requests: RunRequest[] = [];
  await mountAgentControl(requests);
  await enterText(
    requireControl<HTMLTextAreaElement>("textarea"),
    "Inspect the work area",
  );
  await selectOption(providerSelect(), "OpenAI");
  await selectOption(modelSelect(), "Other");

  expect(startButton().disabled).toBe(true);
  await enterText(customModelInput(), " gpt-custom-preview ");
  expect(readViewerPreferences(localStorage).agentModelSelection).toEqual({
    provider: "openai",
    model: " gpt-custom-preview ",
    custom: true,
  });

  await mountAgentControl(requests);
  expect(providerSelect().value).toBe("openai");
  expect(modelSelect().selectedOptions[0]?.label).toBe("Other");
  expect(customModelInput().value).toBe(" gpt-custom-preview ");
  await act(async () => startButton().click());
  expect(requests).toEqual([
    {
      arm: "follower",
      task: "Inspect the work area",
      lessons: [],
      cameras: [],
      optionalCameras: [],
      settings: {
        provider: "openai",
        model: "gpt-custom-preview",
        reasoningEffort: "high",
        inferenceMode: "standard",
      },
    },
  ]);
});

test("changing providers clears a custom model and requires a fresh selection", async () => {
  const requests: RunRequest[] = [];
  await mountAgentControl(requests);
  await enterText(
    requireControl<HTMLTextAreaElement>("textarea"),
    "Inspect the work area",
  );
  await selectOption(providerSelect(), "OpenAI");
  await selectOption(modelSelect(), "Other");
  await enterText(customModelInput(), "gpt-custom-preview");
  expect(startButton().disabled).toBe(false);

  await selectOption(providerSelect(), "Gemini");
  expect(modelSelect().value).toBe("");
  expect(view.querySelector('input[name="customModel"]')).toBeNull();
  expect(readViewerPreferences(localStorage).agentModelSelection).toEqual({
    provider: "gemini",
    model: null,
  });
  expect(startButton().disabled).toBe(true);

  await selectOption(modelSelect(), "Gemini 4 Hydrogen");
  expect(readViewerPreferences(localStorage).agentModelSelection).toEqual({
    provider: "gemini",
    model: "gemini-4-hydrogen",
  });
  await act(async () => startButton().click());
  expect(requests).toEqual([
    expect.objectContaining({
      settings: expect.objectContaining({
        provider: "gemini",
        model: "gemini-4-hydrogen",
      }),
    }),
  ]);
});

test("lessons for the typed task are offered, can be switched off and edited, and are submitted", async () => {
  const requests: RunRequest[] = [];
  const asked: string[] = [];
  await mountAgentControl(requests, {
    ...agent,
    loadLessons: async (task) => {
      asked.push(task);
      return [
        {
          runId: "r1",
          outcome: "stopped",
          text: "approach from above",
          savedAt: 1,
        },
        { runId: "r2", outcome: "completed", text: "open wider", savedAt: 2 },
      ];
    },
  });
  await enterText(
    requireControl<HTMLTextAreaElement>("#agent-task"),
    " pick up the tube ",
  );
  await selectOption(providerSelect(), "OpenAI");
  await selectOption(modelSelect(), "GPT-6 Sol");
  await act(() => new Promise((resolve) => setTimeout(resolve, 400)));
  expect(asked.at(-1)).toBe("pick up the tube");
  expect(view.textContent).toContain("Lessons from earlier runs (2 of 2 on)");
  await act(async () =>
    requireControl<HTMLInputElement>("#agent-lesson-r1-on").click(),
  );
  await enterText(
    requireControl<HTMLTextAreaElement>("#agent-lesson-r2-text"),
    "open the gripper wider",
  );
  expect(view.textContent).toContain("Lessons from earlier runs (1 of 2 on)");
  await act(async () => startButton().click());
  expect(requests).toEqual([
    expect.objectContaining({
      task: "pick up the tube",
      lessons: ["open the gripper wider"],
    }),
  ]);
});
