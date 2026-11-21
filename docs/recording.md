# Recording

Recording is optional and disabled by default. It captures station telemetry and
LiveKit camera streams for playback. Camera streaming and robot control do not
require recording to be enabled.

RUA owns recording lifecycle, station telemetry and interruption status.
[LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras/tree/main/native/recorder)
handles video capture/encoding; [MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools)
handles file writing, video export and browser playback primitives. Their guides
cover those components independently of RUA.

## Prerequisites

Installed wheels include the media companion. For source runs, build the
[media companion](../livekit-cameras/README.md#build-and-verification).
Install `ffmpeg` on the station process's `PATH`. See the upstream
[recorder requirements](https://github.com/Hebbian-Robotics/livekit-cameras/tree/main/native/recorder)
for encoder details. FFmpeg is not bundled in the Python wheel.

## Install FFmpeg

Install FFmpeg on the machine running the station, in the environment that
launches `rua serve`. Installing it only on a remote SDK client is insufficient.
The RUA wheel does not bundle FFmpeg.

These instructions install FFmpeg across platforms. The current prebuilt RUA
wheels support Linux x86-64 with glibc 2.39+ (such as Ubuntu 24.04) and
macOS 14+ on Apple Silicon. Installing FFmpeg does not add native
Windows support or support for older systems.

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
station command you use for [your SO-101](getting-started.md#3-verify-manual-control).
You can also set `HEBBIAN_ENABLE_RECORDING=true` in the station's environment.

In the viewer, select **Record** to start recording or enable **Record when
control starts**. Recording continues when control changes hands. Select **Stop
recording** to finish it. Recording is independent of power and lease ownership.
Clients can explicitly end it: the TypeScript reference agent stops recording
when its task ends normally, while cancellation leaves it running. Station
shutdown also finalizes the active recording.
During shutdown, the station refuses new recordings, finalizes the active one,
and waits for the final LiveKit room-state announcement attempt. Announcement
requests have a timeout; failures are logged and do not prevent shutdown.
If the motor controller stops, recording continues while the station remains
available. The recording includes controller availability events and ends with
an incomplete status because arm telemetry has a gap, even if the controller
later reconnects. A recorder whose supervising station exits finalizes as
incomplete when possible. The viewer reports an unsuccessful finalization.
Recording requests are rejected when recording is disabled. Previously saved
recordings remain available for playback. The viewer reports recorder failures;
camera streaming runs separately.
If FFmpeg or LiveKit credentials are missing at startup, the station logs why
recording is unavailable and continues serving control and cameras.

See [media integration](media-integration.md#interpreting-a-recording)
for the recorded signals and their limitations.

## Export and playback

After stopping the capture, open **Recordings** in the viewer and select the
saved recording. The station writes recordings to `recordings/` by default.
Use `--recordings-directory` to choose another location.

Use [MCAP Tools' video exporter](https://github.com/Hebbian-Robotics/mcap-tools#export-mcap-camera-video-to-mp4)
with a RUA camera topic of the form `/cameras/<camera>/video`, using the camera's
registered name. Its guide covers commands,
supported formats and timing limits. RUA's viewer provides station-specific
playback; its range reader, video remuxing and shared clock come from the
[MCAP browser library](https://github.com/Hebbian-Robotics/mcap-tools/tree/main/typescript).

For shared-library changes, update the native pin in `livekit-cameras/Cargo.toml`
and `Cargo.lock`, or the browser pin in `package.json` and `pnpm-lock.yaml`,
according to the affected integration.
