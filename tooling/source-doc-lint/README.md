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
comment exclusions, and the public-interface directories. Ordinary comment
exclusions do not affect the documentation boundary. Generated source is
checked unless the applicable rule or an ignore file excludes it.

Keep useful implementation rationale in a classified comment beside the code.
Put consumer-facing documentation and schema descriptions in the owning public
interface. See the [CLI documentation](https://github.com/Hebbian-Robotics/source-doc-lint)
for supported languages, schema syntax, and configuration options.
