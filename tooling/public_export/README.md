# Reduced source export for the Python wheel

Use this guide to inspect the source subset used by `package rua`. For the full
source release, wheels and publication, see [release preparation](../../docs/releasing.md).

## Assemble a snapshot

From the development repository:

```sh
uv run rua-dev export-public /absolute/path/to/new/rua-export
```

The destination must be new and outside this checkout. The command assembles a
local snapshot; it does not initialize Git, change remotes, commit, push or publish.
It requires Cargo, uv and registry access or populated dependency caches. It
resolves lockfiles but does not compile code.

## Understand what is copied

[`manifest.toml`](manifest.toml) allowlists Git-tracked files. The export copies
their **current working-tree contents**, then applies tracked documentation
and configuration overlays from [`files/`](files/). Untracked files and Git
history are excluded. Use a clean committed source revision for release provenance;
a development export deliberately includes edits to tracked files.

The exporter prunes omitted Cargo/Python workspace members and development
dependencies, then resolves the reduced lockfiles. Shared runtime code, lint
settings, toolchain pins and CI come from the development source. Do not create
public-only copies of runtime code or the development CLI.

The subset contains the Python SDK, launcher, server/media source, connection
profiles and robot-asset pin. It excludes the full viewer, other SDKs/CLI, setup
application and MuJoCo fixture. Keep the allowlist and public docs consistent
when changing that scope. `workspace_commands.py` is omitted; shared tooling
works without that extension.

Robot URDFs/meshes are downloaded and verified by `uv run rua-dev setup`, not
copied from ignored local assets. They are included in wheels according to the
pin. Explicitly allowed tracked license symlinks are flattened to regular files.
Never put private material in allowlisted tracked paths: an allowlist is not a
secret scanner.

## Review the assembled checkout

Read its README and Contributing guide from the destination. Run validation in
a local checkout with the exported files tracked and committed. The comment
checker uses Git ignore rules to skip the virtual environment, and contract
checks compare regenerated files against `HEAD`. For a public refresh, use a
local branch based on the public repository's current history. Check:

- Lockfile changes caused by the reduced workspaces, included profiles and model pins.
- Links and commands in the assembled docs, including overlays; template-relative
  links cannot be checked correctly in this source directory.
- Python/Rust validation, compiled targets, the Python example against the exported
  station, and real LiveKit camera behavior when that integration changes.
- Source contents, dependency/model licenses and absence of private data.

`EXPORT.json` records the source revision, dirty status and exported file hashes.
It is local validation metadata, ignored by Git. Exclude it when copying the
snapshot into a release checkout and remove a previously tracked copy.
