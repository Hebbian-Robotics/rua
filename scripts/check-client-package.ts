import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repositoryDirectory = resolve(import.meta.dirname, "..");
const packageDirectory = join(repositoryDirectory, "typescript-client");
const consumerDirectory = mkdtempSync(join(tmpdir(), "rua-client-package-"));

try {
  execFileSync(
    "pnpm",
    ["pack", "--out", join(consumerDirectory, "client.tgz")],
    {
      cwd: packageDirectory,
      stdio: "inherit",
    },
  );
  writeFileSync(
    join(consumerDirectory, "package.json"),
    JSON.stringify({
      private: true,
      type: "module",
      dependencies: { "@hebbian/robot-use-client": "file:./client.tgz" },
    }),
  );
  execFileSync("pnpm", ["install", "--offline", "--ignore-scripts"], {
    cwd: consumerDirectory,
    stdio: "inherit",
  });
  const installedPackageDirectory = join(
    consumerDirectory,
    "node_modules/@hebbian/robot-use-client",
  );
  const installedPackage = JSON.parse(
    readFileSync(join(installedPackageDirectory, "package.json"), "utf8"),
  );
  assert.equal(installedPackage.exports["."].import, "./dist/index.js");
  assert.equal(installedPackage.exports["."].development, undefined);
  writeFileSync(
    join(consumerDirectory, "consumer.mjs"),
    `import assert from "node:assert/strict";
import { createStationConnection, apiSchemas } from "@hebbian/robot-use-client";
assert.ok(import.meta.resolve("@hebbian/robot-use-client").endsWith("/dist/index.js"));
assert.equal(typeof createStationConnection, "function");
assert.ok(apiSchemas.shape.StationState);
`,
  );
  execFileSync(process.execPath, ["consumer.mjs"], {
    cwd: consumerDirectory,
    stdio: "inherit",
    env: { ...process.env, NODE_OPTIONS: "" },
  });
  writeFileSync(
    join(consumerDirectory, "consumer.mts"),
    `import { createStationConnection, type StationState } from "@hebbian/robot-use-client";
const connection = createStationConnection("http://127.0.0.1:8792", "agent");
const state: Promise<StationState> = connection.state();
void state;
`,
  );
  execFileSync(
    process.execPath,
    [
      join(repositoryDirectory, "node_modules/typescript/lib/tsc.js"),
      "--noEmit",
      "--strict",
      "--skipLibCheck",
      "--target",
      "ES2023",
      "--module",
      "NodeNext",
      "--moduleResolution",
      "NodeNext",
      "consumer.mts",
    ],
    { cwd: consumerDirectory, stdio: "inherit" },
  );
  console.log("Packed SDK imports and typechecks outside the workspace.");
} finally {
  rmSync(consumerDirectory, { recursive: true, force: true });
}
