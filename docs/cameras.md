# Cameras and the web preview

RUA uses [LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras)
for camera streaming and its hosted web preview. RUA starts the media companion
for you; your Python agent needs no LiveKit SDK or frontend build.

## Watch a USB camera

1. Put your LiveKit project's credentials in `.env` where you launch RUA:

   ```sh
   LIVEKIT_URL=wss://your-project.livekit.cloud
   LIVEKIT_API_KEY=your-api-key
   LIVEKIT_API_SECRET=your-api-secret
   ```

2. Start RUA with your robot profile and camera:

   ```sh
   uv run rua serve --profile ./robot.toml --camera table=0
   ```

   `table` is the SDK camera name; `0` is the device index. Device paths such as
   `table=/dev/video0` also work. Repeat `--camera` for more cameras.

3. Request watch-only access on the station machine:

   ```sh
   curl --fail http://127.0.0.1:8792/viewer/api/token
   ```

   Open [camera tools](https://hebbian-robotics.github.io/livekit-cameras/), select
   **Room viewer**, and connect with the returned `server_url` and `token`.
   Use your station's HTTP address if it differs from the default above.

The agent and viewer share the cameras. Keep tokens private; API keys and secrets
stay on the station. See the [Room viewer guide](https://github.com/Hebbian-Robotics/livekit-cameras#room-viewer)
for viewer behavior and the [camera tools guide](https://github.com/Hebbian-Robotics/livekit-cameras#browser-camera)
for browser controls and hosting.

## Add a phone

With RUA running, generate an invite from your agent project:

```sh
uv run python - <<'PYTHON'
import asyncio
from hebbian_rua import StationClient

async def main():
    async with StationClient() as station:
        print((await station.phone_invite("table")).url)

asyncio.run(main())
PYTHON
```

Open the private invite on your phone. Its SDK camera name is `phone_table`.
Follow the [browser camera instructions](https://github.com/Hebbian-Robotics/livekit-cameras#browser-camera)
to start streaming. Set `HEBBIAN_PHONE_PAGE_URL` before starting RUA if you host
that page yourself.

## RUA configuration

- Restart RUA after changing credentials. By default, each start creates a new
  room; request a new viewer token and phone invite after restarting.
- Pass `--room demo` to reuse a room. Use distinct rooms for distinct stations.
- Without LiveKit credentials, local USB and simulator snapshots still work for
  the SDK, but are not published to Room viewer. See [simulator integration](simulators.md)
  for user-owned camera streams.
- Source builds need the [native prerequisites](https://github.com/Hebbian-Robotics/livekit-cameras/blob/main/CONTRIBUTING.md#native-rust),
  then `CC=clang-21 CXX=clang++-21 cargo build -p livekit-cameras` on Linux.
  On macOS, use `CC=clang CXX=clang++ cargo build -p livekit-cameras`.
  Allow camera access for your terminal in macOS System Settings when prompted.
  RUA finds the media binary beside the server in `target/debug`.
  Recording additionally needs [FFmpeg](recording.md).

## Compare native publishing codecs

Set `LIVEKIT_CAMERA_CODEC=h264` before starting the station to request H.264
instead of the default VP8. Set `LIVEKIT_CAMERA_DIAGNOSTICS=true` to report
publishing statistics every five seconds; omit it for normal operation. These
variables apply to native USB publishers, not phone cameras or simulation views.
The media child inherits them from the station environment.

The publisher reports its negotiated codec and encoder once, even with diagnostics
off. H.264 is a request, not a guarantee of hardware encoding. Compare the same
camera setup under similar load and inspect the actual encoder and mean encoding
time. Submission age measures host receipt to WebRTC submission, not the complete
camera-to-display delay. A running station must restart to use changed variables;
follow the [hardware restart precautions](hardware.md) for a physical arm.

When invoking `hebbian-robot-use-media publish` directly, the equivalent flags are
`--video-codec h264 --camera-diagnostics`. The implementation and measurement
limitations are documented in the [native publisher guide](https://github.com/Hebbian-Robotics/livekit-cameras/blob/main/native/publisher/README.md#codec-selection-and-diagnostics).
