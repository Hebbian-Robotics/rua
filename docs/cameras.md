# Cameras and the web preview

The station supervises its media companion, which owns USB devices and
publishes their tracks into LiveKit. The media service reads room tracks back
as JPEG snapshots for the Python SDK. The browser subscribes directly to the
same room, so previewing never opens the USB device a second time.

The wheel includes the media companion. You only need your LiveKit project and
the hosted Room viewer; no frontend build or LiveKit SDK is needed in your agent.

## Watch a USB camera

1. Create a [LiveKit project](https://cloud.livekit.io). Put its credentials in
   `.env` in the directory where you launch RUA:

   ```sh
   LIVEKIT_URL=wss://your-project.livekit.cloud
   LIVEKIT_API_KEY=your-api-key
   LIVEKIT_API_SECRET=your-api-secret
   ```

2. Start the station with your robot profile and USB camera:

   ```sh
   uv run rua serve --profile ./robot.toml --camera table=0
   ```

   `table` is the name your agent uses. `0` selects USB camera index 0; you can
   also use a device identifier such as `table=/dev/video0`. Repeat `--camera`
   for more cameras. RUA starts the publisher and snapshot service itself.

3. In another terminal on the station machine, request watch-only room access:

   ```sh
   curl --fail http://127.0.0.1:8792/viewer/api/token
   ```

   Open [camera tools](https://hebbian-robotics.github.io/livekit-cameras/),
   select **Room viewer**, and paste the response's `server_url` and `token`
   into the matching fields. Connect to see the room's camera tracks, including
   cameras that join later. Use your station's actual HTTP address if it differs
   from the default above.

The browser streams video directly from LiveKit while your agent requests
snapshots from the same tracks. Neither the browser nor the snapshot reader opens
the USB camera again.
The station does not open a browser automatically.

Keep the viewer token private; it grants access to the room's media. Your LiveKit
API key and secret stay on the station and are not entered in the browser.

## Rooms and restarts

Restart the station after changing credentials. It chooses a new room on each
start, so request a new viewer token and reconnect after restarting.
`--room demo` keeps one room across restarts. Use distinct rooms for distinct
stations sharing a project.

## Add a phone camera

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

## Hosting and local capture

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

