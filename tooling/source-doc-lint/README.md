# Source Doc Lint

Install the version used in RUA CI with Cargo and a C compiler:

```sh
cargo install --git https://github.com/Hebbian-Robotics/source-doc-lint --tag v0.1.0 --locked
```

Prebuilt binaries are available from the [v0.1.0 release](https://github.com/Hebbian-Robotics/source-doc-lint/releases/tag/v0.1.0).
From the repository root, run:

```sh
source-doc-lint --config source-doc-lint.toml
```

Shared validation and CI run the same check. Append file or directory paths to
narrow the scan or pass `--json` for a structured report.

[RUA's policy](../../source-doc-lint.toml) defines accepted labels, ordinary
comment exclusions, and public-interface paths. Use `SAFETY:` for Rust
unsafe soundness arguments and `ROBOT-SAFETY:` for rules addressing physical
robot hazards. The ordinary comment check covers the SDKs and TypeScript
examples. It excludes generated sources listed in the policy, typing stubs,
and Python teaching examples. Those exclusions apply only to ordinary comments.

Keep useful implementation rationale in a classified comment beside the code.
Put consumer-facing documentation and schema descriptions in the owning public
interface. See the [CLI documentation](https://github.com/Hebbian-Robotics/source-doc-lint)
for supported languages, schema syntax, and configuration options.
