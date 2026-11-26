# Cameras

Set up cameras with LiveKit to watch live video, add a phone camera, or use
frames from the room in the CLI and SDK examples. The station publishes
configured USB cameras to its LiveKit room and serves JPEG snapshots by name.

## Connect a USB camera

1. If you run RUA from source, [build the media companion](#rua-configuration)
   once. The Python wheel includes it.
2. For a station built from this checkout, we recommend installing the [LiveKit CLI](https://docs.livekit.io/reference/developer-tools/livekit-cli/).
   Run `lk cloud auth` as the OS user who runs the station
   and choose your project in the browser. Restart the station after setup.
   RUA reads the default project's credentials from `~/.livekit/cli-config.yaml`.
   If you have several projects, select one with `lk project set-default PROJECT`.
   See [LiveKit CLI authentication](https://docs.livekit.io/reference/developer-tools/livekit-cli/auth/).

   For the published 0.1.7 station or manual setup, open your project in the [LiveKit Cloud dashboard](https://cloud.livekit.io/),
   copy its WebSocket URL, and open the project’s **API keys** page for the key and secret.
   If you missed the secret when creating your account or key, you may need to
   create a new API key. Save its secret when it is shown.
   Set all three variables in the station environment or its
   `.env` file. These take precedence over the CLI configuration. A partial
   set reports the missing variables. For a self-hosted server, use a URL
   reachable over `wss://` for phone cameras.

   ```dotenv
   LIVEKIT_URL=wss://your-project.livekit.cloud
   LIVEKIT_API_KEY=your-api-key
   LIVEKIT_API_SECRET=your-api-secret
   ```

3. For a source checkout, [install FFmpeg](recording.md#install-ffmpeg) or pass
   `--no-recording` to keep LiveKit streaming without recording. Connect the
   cameras and start the station using the
   [source start command](install-from-source.md#start-the-station) or the
   [published wheel command](getting-started.md#2-verify-manual-control).
   For a registered arm whose config includes a wrist-camera selector, RUA
   finds the capture device by USB vendor, product and serial. It waits if
   the camera is disconnected or if several physical devices match. A replug
   can be picked up while the station and viewer are running.

If an active capture stops delivering frames for five seconds, RUA releases
the stale capture process and looks up the USB camera again. LiveKit retries
its connection separately, so a room outage leaves healthy capture running.

Use `rua status --camera wrist` to see if a wrist snapshot is available.
The CLI and viewer distinguish a disconnected camera, multiple USB matches,
stalled capture, and a healthy capture waiting for LiveKit. To add cameras or
override an automatic selection, run these commands on the station host.

```sh
rua camera-devices
rua serve --camera wrist=2 --camera overhead=4
```

Replace `2` and `4` with the `device` values returned for your cameras. Repeat
`--camera NAME=DEVICE` with a distinct name for each device. On Linux, you can
also use a device path or persistent symlink. Pass `--camera wrist=none` to
turn off an automatic camera. The station publishes each configured USB camera
to the same LiveKit room.

From a source checkout with the media companion built, use
`uv run cargo run -p hebbian-rua-server -- camera-devices` and append the
`--camera` options to the source start command. With the arm disabled, briefly
cover the wrist lens and confirm that the `wrist` feed darkens.

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
   curl --fail http://127.0.0.1:8792/media/access-token
   ```

2. Open the [hosted LiveKit Cameras page](https://hebbian-robotics.github.io/livekit-cameras/)
   and select **Room viewer**.
3. Paste the response’s `server_url` and `token` into the URL and token fields,
   then connect. The browser can preview the room's camera tracks while the
   CLI and agents request JPEGs by camera name. Keep the room token private.

Alternatively, paste the same URL and token into the **Custom** tab in
[LiveKit Meet](https://meet.livekit.io/?tab=custom).

Use the camera label you chose in SDK calls or example flags such as
`--camera wrist`. If you have the RUA station viewer installed, you can
also watch the feed under **Workspace**.

## Add a phone

With LiveKit configured, run
`cargo run --locked -p hebbian-rua-cli -- phone-invite overhead` from the
checkout root on the station host and open the printed URL on your phone.
You can also use the HTTP request below and open the response’s `url`. On the
camera page, allow camera access and select **Start streaming**. Check the feed
under **Workspace** in the laptop viewer to confirm its orientation and framing.

The station accepts invite requests over loopback. Run the viewer backend on the
station host and connect it to the station over loopback. In **Workspace**, open
**Camera setup**.
Under **Remote camera**, enter a camera location and select
**Create invite**. On your phone, open the invite link or scan its QR code.
To connect manually, open **Manual LiveKit connection** in the viewer, then
paste its **LiveKit server URL** and **Publishing token** into the camera page
and select **Use connection**. The station creates the room name by default
and issues a one-hour publishing token with each invite. Keep the token private.

```sh
curl --fail http://127.0.0.1:8792/media/phone-invites \
  --json '{"label":"overhead"}'
```

The feed name is `phone_<label>`. An invite labeled `overhead` creates the
`phone_overhead` feed, and an invite labeled `table` creates `phone_table`. See the
[browser camera guide](https://github.com/Hebbian-Robotics/livekit-cameras#browser-camera)
for phone controls and custom hosting.

## Check a camera from the CLI

LiveKit camera names use 1–40 lowercase ASCII letters, digits or underscores.
Simulator snapshots use `sim_` followed by the renderer's view name. Use the
name returned by the camera list for snapshot, observation and lease requests.

With the station running, list available camera names and save a JPEG from
the checkout root:

```sh
cargo run --locked -p hebbian-rua-cli -- camera list
cargo run --locked -p hebbian-rua-cli -- camera snapshot wrist --out wrist.jpg
```

If you installed the release wheel, use [its `hebbian-rua` command](../cli/README.md).
Use the [hosted room viewer](#watch-a-usb-camera) for live playback.

## When LiveKit Cloud is unavailable

Use LiveKit Cloud for normal operation. If the station cannot reach it, the
viewer loses live video and agents lose USB and phone camera snapshots, because
both are read from the LiveKit room. A lease that requires a camera stops the
arms within a few seconds and refuses motion until frames return. The options
below apply to the station from this checkout.

| | Local LiveKit server | Local camera fallback |
| --- | --- | --- |
| Setup | Install and run `livekit-server` | Restart with one flag |
| Live video in the example viewer | Yes, through LiveKit | Yes, as a JPEG preview |
| USB camera snapshots for agents and the CLI | Yes | Yes |
| Phone cameras | No | No |
| Recording | Available by default with FFmpeg | No |

Prefer the local LiveKit server when you can install it, since the station and
viewer run their normal LiveKit path.

### Run a local LiveKit server

Install `livekit-server` from the [LiveKit releases](https://github.com/livekit/livekit/releases)
or with `brew install livekit` on macOS, then start it on the station host:

```sh
livekit-server --dev
```

Point the station's `.env` at it and restart the station with your usual
arguments:

```dotenv
LIVEKIT_URL=ws://127.0.0.1:7880
LIVEKIT_API_KEY=devkey
LIVEKIT_API_SECRET=secret
```

`--dev` listens only on the station host and uses public placeholder keys, so
do not expose it to a network. Open the example viewer on the station host.
Phone invites are refused in this setup because the phone camera page can only
connect to a LiveKit server it reaches over `wss://`. Restore your LiveKit Cloud
values and restart the station when the service returns.

### Preview local cameras during an outage

Restart the station with `rua serve --local-only-no-remote-cameras` and keep
your existing `--camera NAME=DEVICE` arguments. Open or reload the
[example viewer](http://127.0.0.1:8793/viewer/) to see the local feeds under
**Cameras**. You can watch while a CLI command or Python agent controls the arm.
Watching cameras requires neither a model-provider key nor a control lease.
[Example viewer setup](../examples/typescript/viewer/README.md)

Phone camera invites, remote feeds, and recording are disabled in this mode.
To keep LiveKit streaming on a host without FFmpeg, use `rua serve --no-recording`.

The viewer previews each camera by fetching its latest JPEG snapshot several
times a second, from the same USB capture used for model snapshots. Custom
clients can list the local feeds at `GET /media/local-cameras` and fetch frames
from `GET /cameras/NAME`. Simulator views are available as snapshots.

```sh
curl --fail http://127.0.0.1:8792/cameras
curl --fail http://127.0.0.1:8792/cameras/wrist --output wrist.jpg
```

The station limits concurrent forwarded camera requests and returns `503` when
the limit is reached or `504` when the media companion times out.

For direct access, use the companion URL, `http://127.0.0.1:8781` by default.
For direct camera requests, set the Python SDK or viewer's
`HEBBIAN_MEDIA_SERVER` override, set the TypeScript HTTP client's `mediaUrl`,
or pass `--media-server` to the CLI. SDK observations and sequence image
history use station routes. To serve previews and snapshots exclusively from the companion, start the station with
`--disable-camera-forwarding`. Use `--media-address` to choose which address
the companion listens on. The media URL override also lets a new SDK fetch
snapshots from an older station's companion.

## RUA configuration

For source builds, install the [native prerequisites](https://github.com/Hebbian-Robotics/livekit-cameras/blob/main/CONTRIBUTING.md#native-rust)
and build `hebbian-rua-media` from the checkout root. RUA finds the companion
beside the station binary.

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
