import os
import subprocess
import sys
from pathlib import Path, PurePosixPath

ALL_LANES = {"documentation", "comments", "python", "typescript", "rust", "integration"}
RUST_CRATES = {
    "cli",
    "contracts",
    "livekit-cameras",
    "recordings",
    "robot-geometry",
    "sdk",
    "server-core",
    "station",
    "station-access",
}
TYPESCRIPT_CONFIG = {
    ".nvmrc",
    "biome.json",
    "components.json",
    "knip.json",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "tsconfig.json",
}
RUST_CONFIG = {
    "Cargo.lock",
    "Cargo.toml",
    "clippy.toml",
    "rust-toolchain.toml",
}


def file_lanes(filename: str) -> set[str]:
    path = PurePosixPath(filename)
    scope = path.parts[0]
    if path.suffix == ".md" or filename == ".markdownlint-cli2.jsonc":
        return {"documentation"}
    if filename in TYPESCRIPT_CONFIG:
        return {"documentation", "comments", "typescript", "integration"}
    if scope in {"python-bindings", "sdk", "contracts"}:
        return {"comments", "rust", "python", "integration"}
    if scope in RUST_CRATES and path.name == "pyproject.toml":
        return ALL_LANES
    if filename in RUST_CONFIG or scope == ".cargo" or scope in RUST_CRATES:
        return {"comments", "rust", "integration"}
    if path.is_relative_to("examples/typescript"):
        return {"comments", "typescript"}
    if scope == "typescript-client" or filename == "scripts/inline-schema-refs.ts":
        return {"comments", "typescript", "integration"}
    if filename in {
        "vite.config.ts",
        "scripts/build-client.ts",
        "scripts/check-client-package.ts",
    }:
        return {"comments", "typescript"}
    if path.is_relative_to("tests/fixtures"):
        return ALL_LANES
    if path.is_relative_to("examples/mujoco_sim") or scope in {
        "hebbian_rua",
        "tests",
        "typings",
    }:
        return {"comments", "python", "integration"}
    if path.is_relative_to("examples/python") or path.is_relative_to(
        "examples/applications"
    ):
        return {"comments", "python"}
    if path.suffix == ".py" and scope == "scripts":
        return {"comments", "python"}
    return ALL_LANES


def pull_request_lanes() -> set[str]:
    try:
        parents = subprocess.check_output(
            ["git", "rev-list", "--parents", "-n", "1", "HEAD"], text=True
        ).split()
        if len(parents) != 3:
            return ALL_LANES
        # CONSTRAINT: Checkout uses the PR merge commit. Its first parent is the
        # exact base, and the diff covers the entire PR, including file deletions.
        changes = subprocess.check_output(
            ["git", "diff", "--name-status", "--no-renames", "-z", parents[1], "HEAD"]
        )
        if not changes:
            return ALL_LANES
        fields = os.fsdecode(changes.removesuffix(b"\0")).split("\0")
        lanes: set[str] = set()
        for status, filename in zip(fields[::2], fields[1::2], strict=True):
            lanes.update(file_lanes(filename))
            if status != "M":
                lanes.add("documentation")
        return lanes
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(f"Could not classify PR changes: {error}", file=sys.stderr)
        return ALL_LANES


def main() -> None:
    lanes = (
        pull_request_lanes()
        if os.environ["GITHUB_EVENT_NAME"] == "pull_request"
        else ALL_LANES
    )
    print(f"Selected checks: {', '.join(sorted(lanes))}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for lane in sorted(ALL_LANES):
            print(f"{lane}={str(lane in lanes).lower()}", file=output)


if __name__ == "__main__":
    main()
