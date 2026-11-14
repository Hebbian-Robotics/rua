# Rust dependency license files

Some published crates omit license files from their package archives. The files
under `sources/` normally come from the exact upstream commits recorded in
the crate metadata or pinned Git source. `manifest.json` maps each crate and version to
its upstream repository, commit and license files.

The wheel build uses these copies alongside the license files shipped by other
crates. Packaging fails if a dependency or version has no license file. This
inventory does not cover separately installed native libraries.

For `from_variants` and `from_variants_impl` 1.0.2, the exact upstream revision
contains the dual-license declaration but no license text. This distribution
uses its Apache-2.0 option and includes the standard Apache license text, with
its source URL recorded in the manifest and a separate provenance notice.
