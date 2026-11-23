#!/bin/sh
set -eu
root=$(dirname "$(dirname "$5")")
trap 'exit 0' INT TERM
printf ready > "$root/recorder-ready"
while :; do sleep 1; done
