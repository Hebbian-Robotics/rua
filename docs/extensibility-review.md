# Build another application on RUA

Custom applications use the same station APIs as the reference viewer and
agents. They do not need to import the React UI or run the example agent server.

| Application | Public boundary | Starting point |
| --- | --- | --- |
| Monitoring UI | HTTP state and resumable events | [Station reference](reference.md#station-server) |
| Planner or teleoperation | Leases, pose/joint commands, paths, groups and sequences | [Python loop guide](write-your-own-loop.md), [TypeScript SDK](../typescript-client/README.md), [Rust SDK](../sdk/README.md) |
| Agent/framework | Tool definitions, context, execution and output transforms | [Tool selection](write-your-own-loop.md#with-a-model-four-pieces), [model output](model-output.md) |
| Camera application | SDK snapshots or LiveKit publishing | [Cameras](cameras.md) |
| Recording analysis | MCAP and recording HTTP routes | [Recording](recording.md), [media integration](media-integration.md) |
| Simulator | Per-arm plant sockets and generated schema | [Simulator contract](simulators.md) |

## Browser deployment

The TypeScript connection takes a station URL. The reference viewer uses its
own origin for station/playback calls and Vite proxies them during development.
The station does not provide a general configurable browser CORS policy. For a
separately hosted application, use a same-origin reverse proxy for the station
and recording routes. The example agent server has its own origin settings;
those do not configure the station’s CORS behavior.

To replace the viewer served by the station, set `HEBBIAN_VIEWER_DIRECTORY` to
static assets whose paths work under `/viewer/`. Keep control, model credentials
and camera access grants on trusted hosts; the station API has no authentication.

## Control and SDK limits

The station owns admission, lease fencing, limits and interruption handling.
An external planner sends targets rather than running a second hardware servo
loop. There is no general velocity/torque interface or hard-real-time SDK.

SDK convenience coverage differs. TypeScript supports acceptance-only streamed
joint commands with `waitForOutcome: false`, used by manual follow mode. Python
and Rust command helpers wait for the final outcome; they do not currently offer
the same stream-start handle for a sequential teleoperation loop. Use planned
commands in those SDKs unless you implement against the wire API deliberately.

Rust observation tools return state; fetch JPEGs separately. Recording-library
listing/download/delete routes are used directly by the viewer rather than
exposed uniformly as high-level helpers in all SDKs. The generated
[OpenAPI contract](../schemas/openapi.json) describes the underlying routes.

SO-101 is the supported physical driver. A simulator with another model does not
establish hardware support for it. See [embodiment support](architecture.md#embodiments-and-controller-support).

## Cameras and timing

A custom loop may use its own images. For station discovery, required-camera
checks, viewer video and recording, publish into the configured LiveKit room.
Native device capture accepts camera devices, not arbitrary RTSP or file URLs.
The fixture’s camera socket is an internal simulator integration, not a general
frame-upload SDK.

Robot state and snapshots are not synchronized. Capture timestamps help assess
age; they do not establish synchronized multi-camera/robot measurements.

## Hosting a loop

Run a custom loop independently with its own model credentials and station URL.
The example agent server hosts only the TypeScript reference loop; it does not
launch arbitrary user programs. Applications can publish run telemetry without
using that host. See [running from the viewer](write-your-own-loop.md#run-your-program-from-the-viewer).

Validate the boundaries your application uses: disconnects, lease takeover,
cancellation, camera loss and recording finalization. Reference application
coverage does not substitute for checking an independently deployed application.
