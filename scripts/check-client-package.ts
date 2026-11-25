import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repositoryDirectory = resolve(import.meta.dirname, "..");
const packageDirectory = join(repositoryDirectory, "typescript-client");
const consumerDirectory = mkdtempSync(join(tmpdir(), "rua-client-package-"));
const consumerTarball = join(consumerDirectory, "client.tgz");
const releaseTarball = process.argv[2];

try {
  if (releaseTarball) {
    copyFileSync(releaseTarball, consumerTarball);
  } else {
    execFileSync("pnpm", ["pack", "--out", consumerTarball], {
      cwd: packageDirectory,
      stdio: "inherit",
    });
  }
  writeFileSync(
    join(consumerDirectory, "package.json"),
    JSON.stringify({
      private: true,
      type: "module",
      dependencies: { "hebbian-rua": "file:./client.tgz" },
    }),
  );
  execFileSync("pnpm", ["install", "--prefer-offline", "--ignore-scripts"], {
    cwd: consumerDirectory,
    stdio: "inherit",
  });
  const installedPackageDirectory = join(
    consumerDirectory,
    "node_modules/hebbian-rua",
  );
  const installedPackage = JSON.parse(
    readFileSync(join(installedPackageDirectory, "package.json"), "utf8"),
  );
  assert.equal(installedPackage.exports["."].import, "./dist/index.js");
  assert.equal(installedPackage.exports["."].development, undefined);
  writeFileSync(
    join(consumerDirectory, "consumer.mjs"),
    `import assert from "node:assert/strict";
import { createStationConnection, apiSchemas } from "hebbian-rua";
assert.ok(import.meta.resolve("hebbian-rua").endsWith("/dist/index.js"));
assert.equal(typeof createStationConnection, "function");
assert.ok(apiSchemas.shape.StationState);
const stationUrl = process.env.RUA_PACKAGE_CHECK_STATION;
if (stationUrl) {
  const connection = createStationConnection(stationUrl, "agent");
  try {
    connection.start();
    const state = await connection.state();
    assert.equal(state.arms.length, 1);
    assert.equal(state.arms[0].mode, "holding");
    const lease = await connection.acquireLease("installed SDK quickstart", {
      requiredCameras: ["sim_overview"],
    });
    try {
      const observation = await lease.waitForObservation(
        { required_cameras: ["sim_overview"] }, state.arms[0].arm, 5000,
      );
      assert.equal(observation.status, "observation_captured");
      assert.equal(observation.state.arms[0].mode, "holding");
      assert.deepEqual(observation.state.arms[0].interlocks, []);
      const jpeg = Buffer.from(observation.cameras.sim_overview.jpeg_base64, "base64");
      assert.equal(jpeg.readUInt16BE(0), 0xffd8);
    } finally {
      await lease.return();
    }
    const recording = await fetch(new URL("/recording/enabled", stationUrl));
    assert.equal(await recording.json(), false);
  } finally {
    connection.close();
  }
}
`,
  );
  execFileSync(process.execPath, ["consumer.mjs"], {
    cwd: consumerDirectory,
    stdio: "inherit",
    env: { ...process.env, NODE_OPTIONS: "" },
  });
  writeFileSync(
    join(consumerDirectory, "consumer.mts"),
    `import { createStationConnection, type StationState } from "hebbian-rua";
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
