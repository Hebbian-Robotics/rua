# Camera integration

The media companion connects station cameras and SDK snapshots to the separate
[LiveKit camera tools](https://github.com/Hebbian-Robotics/livekit-cameras).
See [camera setup](../docs/cameras.md) for the web preview.

## Build and verification

See [camera setup](../docs/cameras.md#rua-configuration) for prerequisites and
build instructions. Recording additionally needs [FFmpeg](../docs/recording.md).

Run the companion with `--help` to inspect its standalone subcommands. When using both publishing and snapshots,
let one process open the physical camera and subscribe to the room for snapshots.

Run `cargo test --locked -p livekit-cameras` to exercise synthetic video and telemetry
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
follow the [hardware restart precautions](../docs/hardware.md) for a physical arm.

When invoking `hebbian-robot-use-media publish` directly, the equivalent flags are
`--video-codec h264 --camera-diagnostics`. The implementation and measurement
limitations are documented in the [native publisher guide](https://github.com/Hebbian-Robotics/livekit-cameras/blob/main/native/publisher/README.md#codec-selection-and-diagnostics).

## Integration code

| Concern | Source |
| --- | --- |
| Child process lifecycle | [Media supervision](../station/src/media.rs), [recording supervision](../station/src/recording.rs) |
| Snapshot routes, camera names and expiry | [Snapshots](../livekit-cameras/src/snapshots.rs) |
| Station telemetry, MCAP topic mapping and resume | [Station stream adapter](../livekit-cameras/src/station.rs) |
| Video channels and archive layout | [Recorder output](../livekit-cameras/src/recorder/video_output.rs), [archive](../livekit-cameras/src/recorder/archive.rs) |
| Presentation and playback | [Recording library](../recordings/src/library.rs) |
