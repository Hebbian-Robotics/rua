# Recording

Recording is optional and disabled by default. It captures station telemetry and
LiveKit camera streams for playback. Camera streaming and robot control do not
require recording to be enabled.

RUA owns recording sessions, station telemetry and interruption status.
[LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras/tree/main/native/recorder)
handles video capture/encoding; [MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools)
handles file writing, video export and browser playback primitives. Their guides
cover those components independently of RUA.

## Prerequisites

Installed wheels include the media companion. For source runs, build the
[media companion](../livekit-cameras/README.md#build-and-verification).
Install `ffmpeg` on the station process’s `PATH`.

## Install FFmpeg

Install FFmpeg on the machine running the station, in the environment that
launches `rua serve`. Installing it only on a remote SDK client is insufficient.
Install it separately when using the RUA wheel.

### Ubuntu / Debian

```sh
sudo apt update
sudo apt install ffmpeg
```

### macOS

With [Homebrew](https://formulae.brew.sh/formula/ffmpeg) installed:

```sh
brew install ffmpeg
```

### Windows with WSL2

Install FFmpeg inside the Ubuntu environment running the station, using the
Ubuntu commands above. A Windows `ffmpeg.exe` installation does not provide the
Linux executable needed by RUA in WSL. See [WSL setup](windows-wsl.md).

### Other Linux distributions

Use your distribution's FFmpeg package or a build linked from the
[FFmpeg download page](https://ffmpeg.org/download.html). Choose a build with
the `libx264` encoder; some distribution packages omit it.

### Verify before starting the station

Run these commands in the terminal that will launch the station (the same
commands work in PowerShell):

```sh
ffmpeg -version
ffmpeg -hide_banner -encoders
ffmpeg -hide_banner -f lavfi -i color=size=64x64:rate=1 -frames:v 1 -c:v libx264 -f null -
```

The encoder list must include `libx264`, and the final command must finish
successfully. It encodes one synthetic frame without writing a file or using a
camera. The recorder uses `libx264` as its portable encoder; Linux can also use
Intel VAAPI when available.

If `ffmpeg` is not found, add its executable directory to `PATH` and reopen the
terminal. For a service or container, install FFmpeg and configure `PATH` inside
that environment too, then restart the station so it sees the change. Support
and disable a connected arm before restarting its station.

## Enable recording

Configure [LiveKit cameras](cameras.md), then add `--enable-recording` to the
station start command or set `HEBBIAN_ENABLE_RECORDING=true`.

Call `await station.ensure_recording()` with the Python SDK to start or join
a recording. The TypeScript equivalent is `createRecordingClient(url).ensure()`.
Both calls require a station that supports `POST /recording/ensure`.
Concurrent callers receive the same recording name. If a recording is stopping,
wait for it to finish before retrying.

Finish a recording with `await lease.stop_recording()` while holding the lease.
In the optional viewer, use **Record**, **Record when control starts** or
**Stop recording**.

Recording continues across lease handover. The TypeScript reference agent can
stop recording on normal completion. Cancelling its run leaves recording
active. Station shutdown finalizes the active recording.

During shutdown, the station refuses new recordings, finalizes the active one,
and waits for the final LiveKit room-state announcement attempt. Announcement
requests have a timeout; failures are logged and do not prevent shutdown.
If the motor controller stops, recording continues while the station remains
available. The recording includes controller availability events and ends with
an incomplete status because arm telemetry has a gap, even if the controller
later reconnects. When the archive finalizes successfully, the station reports
`finished` with the `controller_interrupted` warning, and the viewer reports
that the recording was saved with a telemetry gap. A `.controller-interrupted`
file means the station observed a controller interruption. Use it to flag the
telemetry gap in clip tooling and use the MCAP's `/recording/status` channel to
read the archive's outcome. A recorder whose supervising station exits finalizes
as incomplete when possible.
Recording requests are rejected when recording is disabled. Previously saved
recordings remain available for playback. The viewer reports recorder failures;
camera streaming runs separately.
If FFmpeg or LiveKit credentials are missing at startup, the station logs why
recording is unavailable and continues serving control and cameras.

The recorder starts with a station snapshot and follows one ordered event stream
for arm and agent telemetry. It resumes from the last archived event after a
disconnect. If the station can no longer replay from that event, the recorder
writes a gap marker and starts from a fresh snapshot.

See the [recording format](../recordings/README.md#recording-format) for
topics, timestamps and gaps.

## Export and playback

The station writes recordings to `recordings/` by default. Use
`--recordings-directory` to choose another location. If you run the example
viewer, open **Recordings** and select the saved recording.

Use [MCAP Tools' video exporter](https://github.com/Hebbian-Robotics/mcap-tools#export-mcap-camera-video-to-mp4)
with a RUA camera topic of the form `/cameras/<camera>/video`, using the camera's
registered name. Its guide covers commands,
supported formats and timing limits. RUA's viewer provides station-specific
playback; its range reader, video remuxing and shared clock come from the
[MCAP browser library](https://github.com/Hebbian-Robotics/mcap-tools/tree/main/typescript).

For shared-library changes, update the native pin in `livekit-cameras/Cargo.toml`
and `Cargo.lock`, or the browser pin in `package.json` and `pnpm-lock.yaml`,
according to the affected integration.
