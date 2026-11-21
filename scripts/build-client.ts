import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import { resolve } from "node:path";

const packageDirectory = resolve(import.meta.dirname, "../typescript-client");
rmSync(resolve(packageDirectory, "dist"), { recursive: true, force: true });
execFileSync("pnpm", ["exec", "tsrs", "-p", "tsconfig.build.json"], {
  cwd: packageDirectory,
  stdio: "inherit",
});
