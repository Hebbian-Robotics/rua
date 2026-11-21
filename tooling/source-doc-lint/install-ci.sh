#!/usr/bin/env bash
set -euo pipefail

version=v0.1.0
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
  Darwin:arm64) target=aarch64-apple-darwin ;;
  Darwin:x86_64) target=x86_64-apple-darwin ;;
  *)
    printf 'No source-doc-lint binary for %s:%s\n' "$(uname -s)" "$(uname -m)" >&2
    exit 1
    ;;
esac

installer_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
binary_directory="$RUNNER_TEMP/source-doc-lint"
archive="source-doc-lint-$target.tar.gz"
mkdir -p "$binary_directory"
cd "$binary_directory"
curl --fail --location --retry 3 --silent --show-error \
  "https://github.com/Hebbian-Robotics/source-doc-lint/releases/download/$version/$archive" \
  --output "$archive"
awk -v archive="$archive" '$2 == archive { print; found = 1 } END { exit !found }' \
  "$installer_directory/SHA256SUMS" | shasum -a 256 --check
tar -xzf "$archive" source-doc-lint
printf '%s\n' "$binary_directory" >> "$GITHUB_PATH"
