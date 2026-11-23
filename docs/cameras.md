# Cameras

Set up cameras with LiveKit to watch live video, add a phone camera, or use
frames from the room in the CLI and SDK examples. The station publishes
configured USB cameras to its LiveKit room and serves JPEG snapshots by name.

## Connect a USB camera

1. If you run RUA from source, [build the media companion](#rua-configuration)
   once. The Python wheel includes it.
2. Create a [LiveKit Cloud project](https://cloud.livekit.io/) and generate a project API key, or use a reachable self-hosted
   LiveKit server. Put its WebSocket URL and API credentials in the station’s
   `.env` file.

   ```dotenv
   LIVEKIT_URL=wss://your-project.livekit.cloud
   LIVEKIT_API_KEY=your-api-key
   LIVEKIT_API_SECRET=your-api-secret
   ```

3. Connect the wrist camera and start the station. For a registered arm whose
   config includes a wrist-camera selector, RUA finds the capture device by
   USB vendor, product and serial. It waits if the camera is disconnected
   or if several physical devices match. A replug can be picked up while
   the station and viewer are running.

If an active capture stops delivering frames for five seconds, RUA releases
the stale capture process and looks up the USB camera again. LiveKit retries
its connection separately, so a room outage leaves healthy capture running.

Use `rua status --camera wrist` to see if a wrist snapshot is available.
The CLI and viewer distinguish a disconnected camera, multiple USB matches,
stalled capture, and a healthy capture waiting for LiveKit. If you bring your
own camera or several match the selector, run `rua camera-devices` on the
station laptop. Each result includes
a `name` and a `device` value. Pass the chosen device to the station, for
example `--camera wrist=2`. Repeat the flag for each extra camera. On Linux, a
device path or a persistent symlink can also be used. Pass `--camera wrist=none`
to turn off an automatic camera.

From a source checkout with the media companion built, use
`cargo run -p hebbian-robot-use-server -- camera-devices`. With the arm disabled,
briefly cover the wrist lens and confirm that the `wrist` feed darkens.

### Registered arm camera selectors

The arm registry can supply shared selectors in `camera-defaults.toml`, keyed
by model variant. Each `arms/<USB-controller-serial>/cameras.toml` can override
an individual camera without changing calibration. For example:

```toml
[soft-gripper-camera]
wrist = "1234:abcd:CAMERA_SERIAL"
```

For a per-arm file, omit the model-variant heading. Use hexadecimal vendor
and product IDs followed by the USB serial. An explicit `--camera` flag takes
precedence over these files.

## Watch a USB camera

1. With LiveKit configured and the station running, get a viewer token on the
   station host:

   ```sh
   curl --fail http://127.0.0.1:8792/viewer/api/token
   ```

2. Open the [hosted LiveKit Cameras page](https://hebbian-robotics.github.io/livekit-cameras/)
   and select **Room viewer**.
3. Paste the response’s `server_url` and `token` into the URL and token fields,
   then connect. The browser connects directly to LiveKit. Keep the room
   token private and leave the LiveKit API key and secret on the station.

Use the camera label you chose in SDK calls or example flags such as
`--camera wrist`. If you have the RUA station viewer installed, you can
also watch the feed under **Workspace**.

## Add a phone

With LiveKit configured, run `hebbian-robot-use phone-invite overhead` on the
station host and open the printed URL on your phone. You can also use the
HTTP request below and open the response’s `url`. On the camera page, allow camera access
and select **Start streaming**. Keep the invite URL private. The station accepts
invite requests over loopback. In the local viewer, open
**Workspace → Add remote camera**. Enter a camera location and select
**Create invite**. On your phone, open the invite link or scan its QR code.
To connect manually, open **Manual LiveKit connection** in the viewer, then
paste its **LiveKit server URL** and **Publishing token** into the camera page
and select **Use connection**. The station creates the room name by default
and issues a one-hour publishing token with each invite. Keep the token private.

```sh
curl --fail http://127.0.0.1:8792/viewer/api/phone-invites \
  --json '{"label":"overhead"}'
```

The feed uses the name `phone_overhead` for that label. See the
[browser camera guide](https://github.com/Hebbian-Robotics/livekit-cameras#browser-camera)
for phone controls and custom hosting.

## Check a camera from the CLI

With the station running, list available camera names and save a JPEG:

```sh
hebbian-robot-use camera list
hebbian-robot-use camera snapshot wrist --out wrist.jpg
```

Use the [hosted room viewer](#watch-a-usb-camera) for live playback.

## Local snapshots

For local cameras such as a built-in webcam or a USB camera, add `--local-cameras-only` to the station’s `serve` command.
Phone invites and room streaming require LiveKit. The media companion captures frames directly. The CLI
and SDKs still use the station URL for discovery and snapshots.

```sh
curl --fail http://127.0.0.1:8792/cameras
curl --fail http://127.0.0.1:8792/cameras/wrist --output wrist.jpg
```

The station limits concurrent forwarded camera requests and returns `503` when
the limit is reached or `504` when the media companion times out. LiveKit
continues to carry live video.

For direct access, use the companion URL, `http://127.0.0.1:8781` by default.
Set the SDK's media URL override or pass `--media-server` to the CLI. To serve
snapshots exclusively from the companion, start the station with
`--disable-camera-forwarding`. Use `--media-address` to choose which address
the companion listens on. The media URL override also lets a new SDK fetch
snapshots from an older station's companion.

## RUA configuration

For source builds, install the [native prerequisites](https://github.com/Hebbian-Robotics/livekit-cameras/blob/main/CONTRIBUTING.md#native-rust)
and build the companion. RUA finds it beside the station binary.

```sh
# Linux
CC=clang-21 CXX=clang++-21 cargo build -p livekit-cameras

# macOS
CC=clang CXX=clang++ cargo build -p livekit-cameras
```

Restart the station after changing credentials. Each start creates a new room
unless you pass `--room NAME`. Give each running station a distinct room.
Recording also needs [FFmpeg](recording.md).

For a custom application, use SDK snapshots or publish compatible tracks to
the station’s LiveKit room for discovery, required-camera checks and recording.
Snapshot timestamps describe capture age. Robot feedback and camera frames
are separate observations.
