import { expect, test } from "vitest";
import { type CommandLine, parseCommandLine } from "../src/cli.js";

function parseRun(
  argv: string[],
  environment: NodeJS.ProcessEnv = {},
): CommandLine {
  const parsed = parseCommandLine(argv, environment);
  if (parsed === "help") throw new Error("expected a command line");
  return parsed;
}

test("the flags of the manual loop command the viewer shows are understood", () => {
  const { config, server } = parseRun(
    [
      "--server",
      "http://station:8792",
      "--camera",
      "chest,wrist_l",
      "--optional-camera",
      "phone_a",
      "--provider",
      "openai",
      "--model",
      "gpt-x",
      "--reasoning-effort",
      "low",
      "--",
      "follower",
      "--pick up the cube",
    ],
    {},
  );
  expect(server).toBe("http://station:8792");
  expect(config).toMatchObject({
    arm: "follower",
    task: "--pick up the cube",
    cameras: ["chest", "wrist_l"],
    optionalCameras: ["phone_a"],
    provider: "openai",
    model: "gpt-x",
    reasoningEffort: "low",
  });
});

test("the inference mode defaults to standard and is refused where the provider or model lacks it", () => {
  expect(
    parseRun(["--provider", "openai", "--model", "gpt-example", "a", "t"])
      .config,
  ).toMatchObject({
    inferenceMode: "standard",
  });
  expect(
    parseRun([
      "--provider",
      "openai",
      "--model",
      "gpt-6-astra",
      "--inference-mode",
      "ultrafast",
      "a",
      "t",
    ]).config.inferenceMode,
  ).toBe("ultrafast");
  expect(() =>
    parseRun([
      "--provider",
      "anthropic",
      "--model",
      "claude-fable-5-2",
      "--inference-mode",
      "ultrafast",
      "a",
      "t",
    ]),
  ).toThrow("not offered by anthropic");
  expect(() =>
    parseRun([
      "--provider",
      "anthropic",
      "--model",
      "claude-sonnet-5-5",
      "--inference-mode",
      "fast",
      "a",
      "t",
    ]),
  ).toThrow("fast is not supported by claude-sonnet-5-5");
});

test("xhigh reasoning effort is opt-in and refused where the model lacks it", () => {
  expect(
    parseRun(["--provider", "openai", "--model", "gpt-example", "a", "t"])
      .config,
  ).toMatchObject({
    reasoningEffort: "high",
  });
  for (const [provider, model] of [
    ["openai", "gpt-6-astra"],
    ["anthropic", "claude-fable-5-2"],
  ] as const)
    expect(
      parseRun([
        "--provider",
        provider,
        "--model",
        model,
        "--reasoning-effort",
        "xhigh",
        "a",
        "t",
      ]).config.reasoningEffort,
    ).toBe("xhigh");
  expect(
    parseRun([
      "--provider",
      "gemini",
      "--model",
      "gemini-4-neon",
      "--reasoning-effort",
      "xhigh",
      "a",
      "t",
    ]).config.reasoningEffort,
  ).toBe("xhigh");
  expect(() =>
    parseCommandLine(
      [
        "--provider",
        "gemini",
        "--model",
        "gemini-4-carbon",
        "--reasoning-effort",
        "xhigh",
        "a",
        "t",
      ],
      {},
    ),
  ).toThrow("xhigh is not supported by gemini-4-carbon");
});

test("provider and model are required even with environment defaults", () => {
  const environment = {
    OPENAI_API_KEY: "test-key",
    OPENAI_MODEL: "gpt-example",
  };
  expect(() =>
    parseRun(["--model", "gpt-example", "Wave"], environment),
  ).toThrow("--provider");
  expect(() => parseRun(["--provider", "openai", "Wave"], environment)).toThrow(
    "--model",
  );
  expect(() =>
    parseRun(["--provider", "openai", "--model", " ", "Wave"], environment),
  ).toThrow("nonempty model ID");
});

test("bad input is refused with a message", () => {
  expect(() =>
    parseCommandLine(
      [
        "--provider",
        "openai",
        "--model",
        "gpt-example",
        "--take-over",
        "follower",
        "Wave",
      ],
      {},
    ),
  ).toThrow(/unknown option.*--take-over/i);
  expect(() =>
    parseCommandLine(["--provider", "openai", "--model", "gpt-example"], {}),
  ).toThrow("Pass <task> or <arm> <task>");
  expect(() =>
    parseCommandLine(
      [
        "--provider",
        "openai",
        "--model",
        "gpt-example",
        "--arm",
        "left",
        "right",
        "Wave",
      ],
      {},
    ),
  ).toThrow("Choose the arm either with --arm or before the task");
  expect(() =>
    parseCommandLine(["--provider", "mistral", "a", "t"], {}),
  ).toThrow(/--provider.*mistral/);
  expect(() =>
    parseCommandLine(
      ["--provider", "openai", "--reasoning-effort", "max", "a", "t"],
      {},
    ),
  ).toThrow("--reasoning-effort");
  expect(() =>
    parseCommandLine(
      [
        "--provider",
        "openai",
        "--model",
        "gpt-example",
        "--camera",
        "a,a",
        "x",
        "t",
      ],
      {},
    ),
  ).toThrow("repeat");
});

test("-h and --help ask for the usage instead of requiring arguments", () => {
  expect(parseCommandLine(["--help"], {})).toBe("help");
  expect(parseCommandLine(["-h"], {})).toBe("help");
});
