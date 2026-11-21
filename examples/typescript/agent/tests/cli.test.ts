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

test("xhigh reasoning effort is opt-in and refused where the model lacks it", () => {
  expect(parseRun(["--provider", "openai", "a", "t"]).config).toMatchObject({
    reasoningEffort: "high",
  });
  for (const provider of ["openai", "anthropic"])
    expect(
      parseRun([
        "--provider",
        provider,
        "--reasoning-effort",
        "xhigh",
        "a",
        "t",
      ]).config.reasoningEffort,
    ).toBe("xhigh");
  expect(
    parseRun(["--provider", "gemini", "--reasoning-effort", "xhigh", "a", "t"])
      .config.reasoningEffort,
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

test("bad input is refused with a message", () => {
  expect(() =>
    parseCommandLine(["--take-over", "follower", "Wave"], {}),
  ).toThrow("Unknown option");
  expect(() => parseCommandLine(["follower"], {})).toThrow(
    "exactly <arm> and <task>",
  );
  expect(() => parseCommandLine(["follower", "Wave"], {})).toThrow(
    "Pass --provider NAME",
  );
  expect(() =>
    parseCommandLine(["--provider", "mistral", "a", "t"], {}),
  ).toThrow("--provider must be one of");
  expect(() =>
    parseCommandLine(
      ["--provider", "openai", "--reasoning-effort", "max", "a", "t"],
      {},
    ),
  ).toThrow("--reasoning-effort");
  expect(() =>
    parseCommandLine(["--provider", "openai", "--camera", "a,a", "x", "t"], {}),
  ).toThrow("repeat");
});

test("-h and --help ask for the usage instead of requiring arguments", () => {
  expect(parseCommandLine(["--help"], {})).toBe("help");
  expect(parseCommandLine(["-h"], {})).toBe("help");
});
