#!/usr/bin/env bash
set -euo pipefail

repository="$(pwd)"
quickstart_directory="$(mktemp -d /tmp/rua-quickstart.XXXXXX)"
station_pid=
simulator_pid=
cleanup() {
  local status=$?
  trap - EXIT
  if (( status != 0 )); then
    for log in station simulator; do
      if [[ -f "${quickstart_directory}/${log}.log" ]]; then
        cat "${quickstart_directory}/${log}.log"
      fi
    done
  fi
  if [[ -n "$simulator_pid" ]]; then kill "$simulator_pid" 2>/dev/null || true; fi
  if [[ -n "$station_pid" ]]; then kill "$station_pid" 2>/dev/null || true; fi
  if [[ -n "$simulator_pid" ]]; then wait "$simulator_pid" || true; fi
  if [[ -n "$station_pid" ]]; then wait "$station_pid" || true; fi
  rm -rf "$quickstart_directory"
}
trap cleanup EXIT

wheel="$(uv run --no-sync rua-dev package rua | tail -n 1)"
tarball="${quickstart_directory}/client.tgz"
pnpm --dir typescript-client pack --out "$tarball"

environment="${quickstart_directory}/environment"
UV_PROJECT_ENVIRONMENT="$environment" uv sync --project examples/mujoco_sim --locked --no-install-package hebbian-rua
uv pip install --python "${environment}/bin/python" "$wheel"

export PATH="${environment}/bin:${PATH}"
cd "$quickstart_directory"
media_port="$(python -I -c 'import socket; listener = socket.socket(); listener.bind(("127.0.0.1", 0)); print(listener.getsockname()[1])')"
rua serve --local-only-no-remote-cameras \
  --profile "$repository/examples/mujoco_sim/profiles/single.toml" \
  --work-directory "$quickstart_directory" --http-address 127.0.0.1:0 \
  --media-address "127.0.0.1:$media_port" --simulator-connect-seconds 120 --simulator-reply-seconds 0.2 >station.log 2>&1 &
station_pid=$!

wait_until() {
  local deadline=$(( SECONDS + 90 ))
  until "$@"; do
    kill -0 "$station_pid"
    if [[ -n "$simulator_pid" ]]; then kill -0 "$simulator_pid"; fi
    (( SECONDS < deadline )) || return 1
    sleep 0.2
  done
}
wait_until test -S arm.plant.sock
python "$repository/examples/mujoco_sim/run.py" --socket-directory "$quickstart_directory" >simulator.log 2>&1 &
simulator_pid=$!

station_announced() {
  HEBBIAN_STATION_URL="$(sed -n 's/^HTTP on //p' station.log | head -n 1)"
  [[ -n "$HEBBIAN_STATION_URL" ]]
}
station_reporting() {
  curl --fail --silent --max-time 2 "${HEBBIAN_STATION_URL}/state" |
    python -I -c 'import json, sys; sys.exit(not any(arm["measured_joints"] for arm in json.load(sys.stdin)["arms"]))'
}
wait_until station_announced
export HEBBIAN_STATION_URL
wait_until station_reporting
timeout 90 python -I "$repository/examples/python/hello_robot.py"
cd "$repository"
RUA_PACKAGE_CHECK_STATION="$HEBBIAN_STATION_URL" pnpm test:package "$tarball"
