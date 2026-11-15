# Recording

Recording is optional. Install `ffmpeg` on the station process's `PATH` when
enabling it; the RUA wheel does not bundle it. Source workspace recording tests
also require this command.

See [LiveKit Cameras' recorder guide](https://github.com/Hebbian-Robotics/livekit-cameras/tree/main/native/recorder)
for encoder requirements and [MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools)
for file utilities and video export. RUA camera topics use
`/cameras/<camera>/video`, for example `/cameras/table/video`.

See [cameras](cameras.md) for RUA's camera configuration. RUA playback and analysis
interfaces are coming soon.
