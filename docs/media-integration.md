# Media integration

The station supervises the media companion and owns recording lifecycle and
access grants. The companion adapts
[LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras)
and [MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools).
The separate binary keeps libwebrtc out of the station and controller builds.

| Concern | Source |
| --- | --- |
| Child process lifecycle | [Media supervision](../station/src/media.rs), [recording supervision](../station/src/recording.rs) |
| Snapshot routes, camera names and expiry | [Snapshots](../livekit-cameras/src/snapshots.rs) |
| Station telemetry, MCAP topic mapping and resume | [Station stream adapter](../livekit-cameras/src/station.rs) |
| Video channels and archive layout | [Recorder output](../livekit-cameras/src/recorder/video_output.rs), [archive](../livekit-cameras/src/recorder/archive.rs) |
| Presentation and playback | [Recording library](../recordings/src/library.rs), [viewer timeline](../examples/typescript/viewer/src/recording-timeline.ts) |

## Interpreting a recording

The archive stores H.264 video as `foxglove.CompressedVideo` on
`/cameras/<camera>/video` and measured joints as ROS 2
`sensor_msgs/msg/JointState` on `/arms/<arm>/joint_states`. The
[archive writer](../livekit-cameras/src/recorder/archive.rs) owns the encodings.

A recording can contain telemetry even when video is unavailable. Check
`/station/recording_errors` and `/recording/metadata` for interruptions. Run
events and span revisions can replay while their sequence history is retained.
Unsequenced control samples and decimated state lost during disconnects leave
gaps. The [cycle trace](controller.md#the-cycle-trace) can provide additional
evidence for that interval.

Arm-frame log times use the controller's emission timestamp. Station snapshots
use recorder arrival time. Run-span log times use server receipt time while the
span payload retains the original start and end. The viewer uses the latest
revision of each span. Camera capture timestamps and robot telemetry do not
establish a synchronized measurement.

## Develop the companion

Follow the [camera build instructions](cameras.md#rua-configuration) and
[FFmpeg setup](recording.md#install-ffmpeg). Run the companion with `--help` to
inspect its standalone subcommands. When using both publishing and snapshots,
let one process open the physical camera and subscribe to the room for snapshots.

Run `cargo test -p livekit-cameras` to exercise synthetic video and telemetry
against a simulated station. Verify device access, encoding, reconnects, and live
playback with the intended camera and host.

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
