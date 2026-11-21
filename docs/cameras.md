# Cameras

LiveKit is the streaming path for RUA. It lets the browser and other clients
watch the same cameras. The station starts the media companion and publishes
your configured cameras to one room. The SDK requests JPEG frames by name.

## Connect a USB camera

1. If you run RUA from source, [build the media companion](#rua-configuration)
   once. The Python wheel includes it.
2. Create or choose a LiveKit project. Copy its WebSocket URL and API
   credentials into `.env` where you start the station.

   ```dotenv
   LIVEKIT_URL=wss://your-project.livekit.cloud
   LIVEKIT_API_KEY=your-api-key
   LIVEKIT_API_SECRET=your-api-secret
   ```

3. Add `--camera wrist=0` to your usual station start command. `wrist` is a
   label you choose, and `0` is the camera device index. You can also use a
   device path such as `/dev/video0`. Repeat the flag for each USB camera.

## Watch a USB camera

Open the station viewer and look for your feed under **Workspace**. Use the
same label in SDK calls or example flags such as `--camera wrist`. If the feed
is missing, check the media status in **Workspace**.

If your install uses the hosted [Room viewer](https://hebbian-robotics.github.io/livekit-cameras/),
run this on the station host, then use the returned `server_url` and `token`
to connect. Keep the token private.

```sh
curl --fail http://127.0.0.1:8792/viewer/api/token
```

## Add a phone

With LiveKit configured, select **Add a phone camera** in **Workspace**, enter
a feed label and scan the QR code. For a station without the viewer, request
an invite and open its private `url` on your phone.

```sh
curl --fail http://127.0.0.1:8792/viewer/api/phone-invites \
  --json '{"label":"overhead"}'
```

The feed uses the name `phone_overhead` for that label. See the
[browser camera guide](https://github.com/Hebbian-Robotics/livekit-cameras#browser-camera)
for phone controls and custom hosting.

## Local snapshots

For an SDK-only setup without LiveKit credentials, register the camera with the
same `--camera` flag. List feeds and check a JPEG using the label you chose.

```sh
curl --fail http://127.0.0.1:8781/cameras
curl --fail http://127.0.0.1:8781/cameras/wrist --output wrist.jpg
```

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
