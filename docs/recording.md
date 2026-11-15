# Recording

Recording is optional. Install `ffmpeg` on the station process's `PATH` when
enabling it; the RUA wheel does not bundle it. Source workspace recording tests
also require this command.

## Install FFmpeg

Install FFmpeg on the machine running the station, in the environment that
launches `rua serve`. Installing it only on a remote SDK client is insufficient.
The RUA wheel does not bundle FFmpeg.

These instructions install FFmpeg across platforms. The current prebuilt RUA
wheel supports Linux x86-64 with glibc 2.39+ (such as Ubuntu 24.04); installing
FFmpeg does not add RUA support for macOS, Windows or older Linux systems.

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

### Windows

In PowerShell with WinGet installed:

```powershell
winget install --id Gyan.FFmpeg --exact
```

Open a new PowerShell window after installation so it picks up the updated
`PATH`. Without WinGet, download a Windows build linked from the
[FFmpeg download page](https://ffmpeg.org/download.html), extract it, and add
the directory containing `ffmpeg.exe` to your user `PATH`.

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

See [LiveKit Cameras' recorder guide](https://github.com/Hebbian-Robotics/livekit-cameras/tree/main/native/recorder)
for encoder requirements and [MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools)
for file utilities and video export. RUA camera topics use
`/cameras/<camera>/video`, for example `/cameras/table/video`.

Each arm's joints are recorded on `/arms/<arm>/joint_states` as ROS 2
`sensor_msgs/msg/JointState`: positions in radians, velocities in radians per
second, and efforts in the hardware profile's effort units. A plant that
reports no velocity or effort leaves that array empty.

See [cameras](cameras.md) for RUA's camera configuration. RUA playback and analysis
interfaces are coming soon.
