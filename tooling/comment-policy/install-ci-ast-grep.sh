#!/usr/bin/env bash
set -euo pipefail

curl --fail --location --retry 3 \
  https://github.com/ast-grep/ast-grep/releases/download/0.40.5/app-x86_64-unknown-linux-gnu.zip \
  --output "$RUNNER_TEMP/ast-grep.zip"
printf '%s  %s\n' \
  '9715cb5933a4d7fe9e4d8c2be870a9a82840c3f2ec4a57bdff7f15d0912cc676' \
  "$RUNNER_TEMP/ast-grep.zip" | sha256sum --check
mkdir -p "$RUNNER_TEMP/ast-grep"
unzip -j "$RUNNER_TEMP/ast-grep.zip" ast-grep -d "$RUNNER_TEMP/ast-grep"
echo "$RUNNER_TEMP/ast-grep" >> "$GITHUB_PATH"
