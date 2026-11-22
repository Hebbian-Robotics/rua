# Release RUA

A release ties the Python distribution and both SDKs to one version and one
source commit. The Python wheels include the station and media companion.

| Registry | Package | Purpose |
| --- | --- | --- |
| PyPI | `hebbian-rua` | Python SDK and station runtime |
| npm | `hebbian-rua` | TypeScript SDK |
| crates.io | `hebbian-rua` | Rust SDK |
| crates.io | `hebbian-rua-contracts` | Shared station wire types, installed automatically by Cargo |

## Prepare and verify

Choose a new version across all three registries. Update the release package
manifests, the contracts dependency version in `sdk/Cargo.toml`, the local
Python dependency pins, and the lockfiles.
Run the version check with the planned tag, for example
`uv run --locked rua-dev check-release-versions --tag v0.1.3`. Regenerate and
verify affected contracts with `uv run --locked rua-dev contracts --check` after
committing the generated files.

Run [Publish](../.github/workflows/publish.yml) manually on a branch or tag that points
to the intended release commit. This builds the Linux and macOS wheels and checks clean installations,
packs and smoke-tests the npm tarball, and builds and verifies both crates
together against a temporary local registry. Download the `rua-sdk-packages`
artifact and keep the run associated with the commit.

Before the first publication, verify ownership of any existing package and
confirm the npm and crates.io accounts can create the new names. Configure the
PyPI publishing credential in the repository's `pypi` GitHub environment.
Build verification runs without publishing credentials.

## First SDK publication

1. Publish `hebbian-rua-contracts` from the verified source commit with
   `cargo publish --locked -p hebbian-rua-contracts`. Wait for the version to
   appear in the crates.io index.
2. Publish `hebbian-rua` from the same commit with
   `cargo publish --locked -p hebbian-rua`. Publish the tested SDK tarball from
   `rua-sdk-packages` to npm with `npm publish <tarball> --access public`.
3. Register GitHub Actions as a trusted publisher in the npm settings for
   `hebbian-rua` and in the crates.io settings for **both** crates. Use these
   values for all three registrations.

   | Setting | Value |
   | --- | --- |
   | GitHub owner | `Hebbian-Robotics` |
   | Repository | `rua` |
   | Workflow filename | `publish.yml` |
   | GitHub environment | `sdk-release` |

   Enable direct publishing in the npm registration. See
   [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers) and
   [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing).

## Publish an approved release

Publish the GitHub release with the matching `v<version>` tag on the verified
release commit. After the build and installation checks pass, the workflow
publishes missing SDK versions using short-lived GitHub Actions credentials.
It uploads the contracts crate before the Rust SDK and publishes the tested npm
tarball. Then it publishes the wheels to PyPI and attaches the packages and
checksums to the GitHub release. Pushing a tag alone leaves the registries unchanged.

Registry writes happen independently. If a publish job fails, rerun it using
the same release commit. The SDK job checks each registry and skips versions
that are already live.
After publishing, install the exact release from each registry in a clean
project and verify the SDK imports, station startup, and a simulator connection.

Update the Python and TypeScript example dependency pins and lockfiles once the
new version is available on PyPI and npm. Keep release-age exemptions limited to
the RUA SDK so the usual delay still applies to third-party dependencies.
