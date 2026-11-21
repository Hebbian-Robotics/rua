# hebbian-robot-use-sdk

Use this SDK when writing a Rust program that controls a running station.
The crate is unpublished. Add it as a local path dependency from a checkout:

```sh
cargo add --path /path/to/rua/sdk
```

Start a station with [your SO-101 connected](../docs/getting-started.md).
To try an agent first, use the [runnable examples](../examples/README.md);
they do not require implementing a model loop or custom formatter.

Async Rust client for a station: acquire the lease, run commands and action sequences, read
state, preview poses, follow run telemetry, and use the station's HTTP routes
(cameras, recordings, traces, phone invites). It depends on `hebbian-contracts`
and light networking crates only (tokio and reqwest), never on the
executor, station, `LiveKit` or axum. Wire types are re-exported as
`hebbian_robot_use_sdk::contracts`.

```rust
use hebbian_robot_use_sdk::{LeaseRequest, Station, contracts::LeaseHolder};

let station = Station::connect("http://127.0.0.1:8792").await?;
let lease = station
    .acquire_lease(LeaseRequest::new(LeaseHolder::Agent, "my script"))
    .await?; // Err(Error::LeaseHeldElsewhere(holder)) when someone else has it
station.enable("arm").await?; // power needs no lease handle
lease.move_to_joints("arm", vec![0.3, -0.4, 0.6, 0.1, 0.0], None).await?; // motion lives on the lease
let state = station.state().await?; // pushed ten times a second, cached
println!("{:?}", state.arms[0].measured_joints);
lease.return_lease().await?; // or drop it for a best-effort background release
```

For explicit compilation, call `station.compile_action_sequence(&source).await?`.
Match `contracts::CompilationOutcome::Compiled { artifact }` or
`Rejected { diagnostics }`. `station.inspect_compilation(&artifact.compilation_id)`
retrieves the retained artifact; `lease.run_compiled_sequence(id, revisions)`
runs it and waits for the outcome. `start_compiled_sequence` returns the execution
ID immediately. Python and TypeScript expose the start method and use their
existing sequence-wait methods. `continue_compiled_sequence` takes an explicit
`ProgramContinuation` and returns its new execution ID. `Station::import_compilation`
recompiles a serialized inspection artifact against current configuration.
See [compilation and artifact semantics](../docs/control-model.md#compile-before-running).

- One method per command, generated from the contracts. `Station` has the ones that
  need no lease: `enable`, `approve_recovery`, `disable`, `stop`, `clear_fault`.
  `LeaseGuard`, which `acquire_lease` and `take_lease` return, has every command that
  moves the arm: `move_to_joints`, `move_to_pose`, `follow_path`, `stream_joints`,
  `set_gripper`, plus `run_arms`, `start_action_sequence`, `run_action_sequence`,
  `move_to_pose_keeping_orientation` and `rehome`. Power and the lease are independent:
  the lease can be held with the arm disabled, and a move on a disabled arm is refused
  as `NotEnabled` with the way on.
- Requests use HTTP; an event-stream task reconnects using the station cursor.
  Each command submission has a request ID that the station uses to avoid
  executing a repeated submission twice. A command's
  outcome is awaited across reconnects (30 s of unreachable station at most),
  then until a state frame shows it no longer running, sampled after it finished, so a state read
  right after `run` shows the arm at rest.
- The SDK renews the lease until it is released or expires. `take_lease` takes it
  from the holder; `revoke_lease` is the operator's override. `LeaseGuard::lost()`
  resolves after a take, revoke, lapse, disconnect, or a return for an interruption
  (`on_interruption: ReturnLease`); dropping the guard attempts a background release. See
  [the control model](../docs/control-model.md).
- The commands return the finished `CommandRecord` or a typed error: `Refused { reason }`
  (nothing moved), `Incomplete { outcome }` (stopped, tripped, timed out...),
  `Timeout`, `Connection`. Dropping a `run` future requests a stop in the
  background; it does not wait for confirmation that the arm has stopped.
- `Station::connect_with(ConnectOptions { run_after, .. })` also returns the
  run telemetry stream (`RunUpdate`), resumed after a sequence number.
- The `_with` variants (`move_to_joints_with`, `enable_with`, `run_arms_with`, ...) take `CommandOptions { trace_context,
  expected_revisions }` (arm to `ArmState::revision`); once an interlock has been
  raised on an arm since, the command is refused as `StaleRevision`. Motion is
  refused as `Interlocked` while an arm has an interlock.
- `action_sequence_updates()` is a `watch` receiver fed by the station's pushes (the newest
  value, including the snapshot after a reconnect). `cancel_action_sequence` waits for
  the HTTP response.
- Lease-bound requests, on `LeaseGuard`: `report_camera(camera, available)` and
  `stop_arm(arm)` use HTTP and are refused as `lease_held_elsewhere` once the lease
  changed; `stop_recording()` is an HTTP route carrying the token. `Station::ensure_recording`
  starts or joins a recording.
- `camera_names`, `camera_jpeg` (media service, default `http://127.0.0.1:8781`),
  `start_recording`, `stop_recording`, `phone_invite`, `list_traces` and
  `download_trace` (gzip bytes as served) are thin HTTP helpers.

```sh
cargo test -p hebbian-robot-use-sdk   # against an in-process executor on the simulated SO-101
```

## Model-facing tools

Use `RobotTools::new(&lease, "arm")` after acquiring a lease. Its
`definitions()` returns provider-independent names, descriptions and JSON schemas;
`with_descriptions` accepts a map of description overrides. `context().await`
returns station data and robot conventions for an application-owned prompt.
Execute a `RobotToolCall { id, name, arguments }` with
`tools.execute(call, CommandOptions::default()).await`. Match `RobotToolOutcome`
for returned data, invalid input, refusal or incomplete motion. Tool inputs and
`result.model_data()` use millimeters and degrees; `result.data()` and the
underlying station client retain metres and radians. Rust `observe` returns
current state and configured camera JPEGs by default; `source: "last_sequence"`
requests retained sequence images instead. The application attaches labelled
JPEGs from `result.cameras` to its model messages. `Station::camera_jpeg` remains
available for direct snapshot access. `tools.execute_calls(calls, options).await` executes
calls concurrently, waits for every outcome, and returns results in input order.
An unconfirmed call is `RobotToolOutcome::Failed`; inspect the arm before proceeding.
Each call can request a speed; station limits apply in every SDK.

Use `with_cameras(vec!["wrist".into()])` to sample images while model action
sequences execute, independently of recording. `tools.sequence_images()` returns
up to eight selected JPEGs with timing, their call identifier, and notices about
gaps or retention limits. Automatic observations should contain current images;
attach historical images only when requested. The `observe` tool supplies them
in `result.cameras` when called with `{"source": "last_sequence"}`. See
[sequence image history](../docs/model-output.md#images-during-action-sequences)
for sampling, resource limits and timing guarantees.

`preview_pose` takes a `poses` list of objects containing only a `pose`.
Model inputs use `position_mm`, `joints_degrees`, and
`max_joint_speed_degrees_per_second`. Results follow input order without IDs or labels;
the SDK generates and matches transport IDs internally. The direct `Station::preview_poses`
API retains SI units and caller-supplied IDs and labels.

For an existing `lease`, a framework can adapt the definitions and submit a call:

```rust,ignore
use std::collections::BTreeMap;
use hebbian_robot_use_sdk::{CommandOptions, RobotToolCall, RobotTools};
use serde_json::json;

let tools = RobotTools::new(&lease, "arm").with_descriptions(BTreeMap::from([
    ("action_sequence".into(), "Execute ordered robot actions.".into()),
]));
let definitions = tools.definitions();
let context = tools.context().await?;
let result = tools.execute(RobotToolCall {
    id: "1".into(),
    name: "action_sequence".into(),
    arguments: json!({"steps": [{"gripper_opening": 0.5}]}),
}, CommandOptions::default()).await?;
println!("{definitions:?} {context:?} {result:?}");
```

Model tools default to sequences, pose previews, observation and immediate stop.
Keep those defaults unless your application needs a different tool set. To customize
it, transform typed capability definitions before schema generation and parsing:

```rust,ignore
use hebbian_robot_use_sdk::contracts::{filter_sequence_actions, SequenceAction};

let tools = RobotTools::new(&lease, "arm")
    .map_tools(filter_sequence_actions(|action| action != SequenceAction::Joints))?;
```

`map_tools` takes `FnOnce(Vec<RobotToolCapability>) -> Vec<RobotToolCapability>`.
Each mapping receives the previous mapping's capabilities, in the selected order;
definitions retain that order.
Use iterator `filter` and `map` to select whole tools or change a sequence's
`SequenceScope`. Invalid configurations return an error. Clone a view before
mapping when both interfaces are needed; views borrow the same lease and do not
open another connection. To replace the preset, return a selection from
`all_robot_tool_capabilities()`. Individual SDK methods remain available.

### Raw results and model views

Tool results retain complete station payloads in original SI units.
Use `result.model_data()` for the default concise model view; no custom formatter
is needed. Customize presentation only when your application needs it; see
[model output](../docs/model-output.md#typescript-and-rust) for the language-specific
accessors and composition examples.

For a custom model view, compose payload transformations before serializing:

```rust,ignore
use hebbian_robot_use_sdk::{ConciseOptions, concise_output, model_response};

let selected = concise_output(result.data(), ConciseOptions::default());
let model_payload = model_response(selected);
let text = serde_json::to_string(&model_payload)?;
```

Both stages take and return owned JSON. Field selection preserves SI units;
conversion belongs at the end. `result.data()` retains the complete station data.
