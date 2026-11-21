# Recordings

RUA's recording library lists and serves saved recordings, supports byte-range
reads, and protects active recordings from deletion. See the
[recording guide](../docs/recording.md) for setup and shared MCAP tools.

## Recording format

The archive stores H.264 video as `foxglove.CompressedVideo` on
`/cameras/<camera>/video` and measured joints as ROS 2
`sensor_msgs/msg/JointState` on `/arms/<arm>/joint_states`. The
[archive writer](../livekit-cameras/src/recorder/archive.rs) owns the encodings.

Joint positions are in radians and velocities in radians per second. Effort
uses the hardware profile’s units. Unreported velocity or effort arrays are
empty.

A recording can contain telemetry even when video is unavailable. Check
`/station/recording_errors` and `/recording/metadata` for interruptions. Run
events and span revisions can replay while their sequence history is retained.
Unsequenced control samples and decimated state lost during disconnects leave
gaps. The controller’s cycle trace can provide additional evidence for that
interval.

Arm-frame log times use the controller's emission timestamp. Station snapshots
use recorder arrival time. Run-span log times use server receipt time while the
span payload retains the original start and end. Camera capture timestamps and robot telemetry do not
establish a synchronized measurement.
