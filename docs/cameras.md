# Cameras and the web preview

The station supervises its media companion, which owns USB devices and
publishes their tracks into LiveKit. The media service reads room tracks back
as JPEG snapshots for the Python SDK. The browser subscribes directly to the
same room, so previewing never opens the USB device a second time.

The wheel includes the media companion. After configuring LiveKit, launch RUA
from your uv project with a USB camera:

```sh
uv run rua serve --profile ./robot.toml --camera table=0
```

## Live view

`rua serve` opens a browser window with a live feed from every camera once
the media service has one, so whoever watches the robot sees what its cameras
see. It needs no LiveKit: the page polls each camera's newest frame from the
media service, shows the frame rate, and keeps retrying while the station
restarts. Pass `--no-view` to skip it, for example on a headless machine. To open
it again, run `uv run python -m hebbian_rua.live_view`.

## Configure LiveKit

Create your own [LiveKit project](https://cloud.livekit.io) and copy its URL,
API key and secret into `.env` in the launch directory:

```sh
LIVEKIT_URL=wss://your-project.livekit.cloud
LIVEKIT_API_KEY=your-api-key
LIVEKIT_API_SECRET=your-api-secret
```

Restart the station after changing credentials. It chooses a new room on each
start; `--room demo` keeps one room across restarts. Use distinct rooms for
distinct stations sharing a project.

Add `--camera table=0` to the station command for USB camera index 0, or use a
device identifier such as `--camera table=/dev/video0`. Repeat the flag for
other cameras. The station starts the publisher and snapshot service itself.

To publish a phone instead, run this from your uv project while the station
is running with LiveKit configured:

```sh
uv run python - <<'PY'
import asyncio
from hebbian_rua import StationClient

async def main():
    async with StationClient() as station:
        print((await station.phone_invite("table")).url)

asyncio.run(main())
PY
```

Open the returned invite URL on the phone and allow camera access. Its SDK
camera name is `phone_table`. The default invite uses the hosted camera tools
page. Invites contain a publishing token valid for 12 hours; use a trusted page
and keep the invite private. It connects directly to LiveKit, not your station.

## Watch the room

Open [camera tools](https://hebbian-robotics.github.io/livekit-cameras/) and select
**Room viewer**. Request a subscribe-only token locally:

```sh
curl --fail http://127.0.0.1:8792/viewer/api/token
```

Paste the response's `server_url` and `token` into the viewer. Keep
the token private. No LiveKit SDK or frontend build is needed in your example.
Use your station's actual HTTP address when it differs from the default.

The camera tools live in the separate public
[livekit-cameras repository](https://github.com/Hebbian-Robotics/livekit-cameras).
You can host that app yourself and set `HEBBIAN_PHONE_PAGE_URL` for invites.

Without LiveKit credentials, the media service captures local USB cameras for
SDK snapshots, and simulator views always come from the simulator socket.
Neither path provides tracks to Room viewer. The server continues controlling
the arm if media is unavailable; camera-dependent agent requests may be refused.

## From source

On Linux install `libglib2.0-dev`, clang/clang++ 21 or newer and the matching
libclang development package. The source checkout uses mold. On Ubuntu 24.04,
LLVM's [APT repository](https://apt.llvm.org/) provides the clang 21 packages.
With those prerequisites installed:

```sh
CC=clang-21 CXX=clang++-21 cargo build -p livekit-cameras
```

The binary is created beside the server in `target/debug`, where the station
finds it automatically. These native prerequisites apply even when recording
is disabled. FFmpeg is additionally required for recording, but not camera viewing.
The media build and camera path are not validated on macOS or Windows.

FFmpeg is an external command used for recording. Install it separately when
you need recording; it is not required just to watch cameras.
