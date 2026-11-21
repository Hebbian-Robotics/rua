# Release RUA

The full source release includes the viewer, all three SDKs, the CLI and the
MuJoCo example. The `hebbian-rua` wheel provides the Python SDK,
Rust station, media companion, SO-101 profiles and pinned geometry.
Provider SDKs are optional extras. Internal server and media wheels are build
inputs for the Python distribution.

## Prepare the source

Bring public contributions back into the development repository first. For the
full source release, use `git archive <revision>` to exclude the preview history,
which contains hardware traces. Review the snapshot for private data before
adding it to the public repository. See the
[wheel export guide](../tooling/public_export/README.md) for the wheel build inputs.

Use a clean committed source revision for a release. Keep package versions
consistent across their manifests; the release tag must be `v` followed by the
version in `pyproject.toml`. Regenerate and commit affected contracts before
running their reproducibility checks.

## Build and verify

```sh
uv run rua-dev package rua
uv run rua-dev package check /absolute/path/to/built-release.whl
```

Replace the second argument with the exact wheel produced by the build. Avoid
wildcards when `dist/` contains several versions or platforms.

In the full checkout, `package rua` first assembles the reduced wheel inputs.
In the reduced checkout it builds directly from that source.
The command builds matching server/media wheels in a temporary directory and
assembles them with the Python package. To reuse component artifacts, supply
both `--server-wheel PATH` and `--media-wheel PATH`; versions and native platform
tags must match.

`robots/so101-assets.toml` lists the SO-101 geometry and notices bundled in the
wheel. Recording requires FFmpeg on the station's `PATH`.

`package check` installs the wheel and its third-party dependencies in an empty
environment, exercises native commands and connects an independent plant-socket
peer. It checks packaging and transport, not simulator physics or hardware.
Also check provider tool calls/images and real LiveKit streams for changes that
affect those integrations.

## Verify the release workflow

A manual run builds, repairs and checks wheels without publishing. Use the
[publish workflow](../.github/workflows/publish.yml) for the supported build
matrix, Python version, and repair tools. The package's minimum Python version
is declared in [pyproject.toml](../pyproject.toml).

Inspect the repaired wheels’ tags, bundled libraries and third-party notices.
New platforms need matching build and clean-install checks.

The publishing job uses the `pypi` GitHub environment and its `PYPI_API_TOKEN`.
Build jobs and manual runs do not use that environment. Check repository access,
PyPI ownership and publishing credentials before triggering a release.

## Publish and confirm

Publishing a GitHub release is the explicit trigger for building, checking and
uploading the verified wheels to PyPI. Pushing a tag alone does not publish.
The workflow also attaches wheels and `SHA256SUMS` to the GitHub release.

Keep the release in draft until its tag points to the intended public-source
commit and the manual workflow/artifact review is complete. Publish only when
release publication is authorized. A development CLI build or export never
publishes a package or changes repository visibility.

After publication, install from the registry in a fresh project with
`uv add "hebbian-rua[openai,anthropic,gemini]"`. Confirm `uv run rua --help`,
station startup and the agent examples outside the development checkout.
