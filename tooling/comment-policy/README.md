# Comment checker

Install ast-grep once (or use `cargo binstall` for a prebuilt binary):

```sh
cargo install ast-grep --version 0.40.5 --locked
```

Run the required check from the repository root:

```sh
uv run python tooling/comment-policy/check.py
```

Shared validation and CI run this command. Failures explain why the checker
requires a special reason for implementation comments and how to fix each
finding. Add paths to narrow a scan, or `--json` for a report containing `reason`
and `findings`, plus `error` when the checker cannot run.

The [classification rules](rules/classified.yml) define the checked languages
and exemptions, including examples and public interfaces. Consecutive standalone
line comments at the same indentation form one paragraph; inline and block
comments are checked independently.

Python docstrings, Rust documentation comments and `doc` attributes, and
TypeScript/TSX JSDoc are permitted only in the repository-relative directories in
[public-interfaces.toml](public-interfaces.toml). Generated source follows the same
boundary. Public types whose documentation feeds a published schema belong in
`contracts/`; command definitions supplying CLI help belong in the listed
command directories. Preserve non-obvious implementation reasons as classified
ordinary comments rather than relocating them into schema metadata.

The [schema rules](rules/schema-metadata.yml) and
[binding analysis](schema_metadata.py) also check these descriptions outside that
boundary:

- Pydantic `Field(description=...)` and literal `json_schema_extra` dictionaries
  containing `description`, on `Field` or `ConfigDict`. Direct, qualified and
  aliased imports are recognized, including imports inside functions; parameter
  and assignment shadowing suppress uncertain bindings.
- Schemars `#[schemars(description = ...)]`, including other attributes alongside
  the description. Rust `#[doc = ...]` is documentation; `#[doc(hidden)]` is not.
- Zod `.describe(...)` and literal `.meta({ description: ... })` on recognized
  schema expressions. Static named, namespace and default imports from `zod`,
  `zod/v3` and `zod/v4`, common schema factories, chained calls and local schema
  aliases are recognized.

These are syntax checks, not whole-program type analysis. They do not follow
re-exports, dynamic imports, computed properties, wrapper functions, metadata
supplied through variables/spreads/callbacks, or Rust `cfg_attr`/Schemars
`extend` attributes. Zod binding analysis is file-local and assumes unshadowed
imports and schema aliases; combined default/named imports and comments inside
import lists are not resolved. Pydantic assignment aliases, star imports,
`global`/`nonlocal` rebinding and comprehension scopes are not resolved. Review
such forms explicitly. Ordinary application fields named `description`, CLI
help, unrelated `.describe()` calls and generators copying public metadata are
not themselves violations.

The scan follows ast-grep's normal ignore handling, except hidden files are
included. Build outputs, dependency caches and the separately maintained
`livekit-cameras/` checkout are excluded; owned generated SDK source remains
included.

Run regression checks with:

```sh
uv run pytest -o addopts= tooling/comment-policy/tests/test_check.py
```
