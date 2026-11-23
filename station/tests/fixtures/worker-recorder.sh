#!/bin/sh
set -eu
root=$(dirname "$(dirname "$5")")
trap 'exit 0' INT TERM
printf '%s' "$$" > "$root/leader-pid"
(
  trap '' INT TERM
  exec 3>"$root/worker-pipe"
  printf x >&3
  touch "$root/worker-ready"
  exec sleep 60
) &
while [ ! -e "$root/quit" ]; do sleep 0.02; done
exit 17
