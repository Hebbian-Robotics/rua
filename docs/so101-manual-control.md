# SO-101 manual viewer control

Manual control is available in the viewer for a single SO-101 arm. It uses
the station's control lease, so another program cannot command the arm while the
viewer holds control.

Manual and agent operation use the same motor controller, hardware driver, feedback,
joint limits, trajectory checks and watchdogs.

## Start

For the published station, follow [Getting started](getting-started.md).
The commands below use a source checkout. Follow
[source setup](install-from-source.md#prepare-the-checkout) and
[build the viewer](install-from-source.md#optional-viewer).

### Automatically select a registered SO-101

Use the [SO-101 calibration tool](so101-calibration.md) to save
`arms/<USB-serial>/setup.toml` and `calibration.json` in the local arm
configuration directory. RUA looks in `$XDG_CONFIG_HOME/rua/so101-arm-configs`
(defaulting to `~/.config/rua/so101-arm-configs`). If you use another directory,
set `HEBBIAN_SO101_CONFIG_DIRECTORY` to its absolute path. Startup reads the
local files and works offline.

List connected USB adapters and their registry matches without opening any
serial device:

```sh
uv run cargo run -p hebbian-rua-server -- so101-discover
```

For a previously calibrated arm whose servo settings already match its profile:

```sh
uv run cargo run -p hebbian-rua-server -- serve
```

The station selects the only registered USB adapter, uses its current
serial port, and installs the registry calibration into LeRobot's calibration
directory if absent. Matching local calibration is reused; differing or malformed
local calibration stops startup without being overwritten. The motor controller's
existing motor-register checks still apply before torque can be enabled; config
discovery itself never changes motor registers or applies PID gains.

If several registered arms are connected, select one with
`--so101-serial <USB-serial>`. Duplicate USB serials are refused. To use a
checkout for just one command, pass `--so101-config-directory /absolute/checkout`.
The same options work with `config --profile auto --arm arm` to inspect the
resolved controller config. Automatic selection currently handles one SO-101
arm. Native setup names it `arm`. Existing registered TOML profiles retain
their configured names. Bimanual setups use explicit profiles.

### Tune motion for one registered arm

Leave the optional `arms/<serial>/motion.toml` absent if the arm runs well on the
defaults. To tune one arm, create it beside that arm's `setup.toml` and include only
the values you need to change. This example lowers the speed ceiling and the
acceleration and jerk limits:

```toml
[motion]
max_joint_speed = 1.0 # rad/s

[trajectory]
acceleration_limit = 2.0 # rad/s²
jerk_limit = 8.0 # rad/s³
```

The short form inherits the arm's model profile. If you omit
`default_joint_speed`, the inherited default is capped at the new maximum.
If you set it explicitly, it must be at or below the maximum. Rates must be
positive and finite. Inspect the resolved values with
`config --profile auto --arm arm`, then restart the disabled station.

A tracking trip reports the affected joint, tracking error and allowance,
and commanded and measured joint speeds when available. Individual Feetech IK
moves also report their planned peak and the last adaptive speed request.

An unregistered USB adapter needs its own setup and calibration entry. When
replacing or swapping an adapter, verify which arm it connects to before
reusing calibration.

### Set up each SO-101

#### Calibrate a new arm

Use the [SO-101 calibration tool](so101-calibration.md) to create the registry entry
and calibrate the arm. It records calibration, assembly-specific model alignment
and gripper endpoints. Select the soft-gripper and camera model for that assembly. Apply and verify the servo gains as described
in the setup guide, with the arm supported and the station stopped.

Start the station as above. With torque off, compare each joint and the
gripper against the 3D model, and correct the registry entry's signs, offsets or
endpoints if needed. Restart the disabled station after calibration changes.
The USB adapter serial identifies the registry entry.

To supply your own port and calibration ID, see
[Use an existing LeRobot calibration](hardware.md).

The SO-101 profile includes a provisional `[rest_reference]` with joint positions
and per-joint tolerances observed on one calibrated arm resting on its table.
The wrist-roll tolerance is intentionally broad because wrist rotation does not
determine whether the arm is supported. Compare the 3D model and physical arm
and validate this reference on each assembly; override it in the local profile
if needed. The profile loader's `RestReference::matches` compares measured
joint angles with those tolerances. A matching pose does not establish
physical support, command a move, or change the controller's runtime rest pose
or shutdown behavior.

The selected assembly’s URDF supplies the geometry, including the TPU fingers,
bracket and camera when using the soft-gripper-camera profile. For hardware the controller
converts that arm’s calibration endpoints through its configured signs and
model offsets, and applies those limits to planning, tracking, inverse kinematics, and the
gripper rotation range the preview reports. The bundled URDF limits remain the defaults for simulation. The
controller refuses invalid/missing calibration; startup errors identify the joint
and measured/model limits. A calibration file or profile change requires a
restart of the disabled controller. A two-arm station uses a unique port and
calibration ID per arm. Compose a local profile and substitute the actual ports,
calibration IDs and measured base poses. The signs and offsets are shared, so
both assemblies must use the same joint mapping.

```toml
extends = ["so101", "so101-bimanual-layout"]
name = "two-arms"

[feetech]
ports = { left = "/dev/so101-left", right = "/dev/so101-right" }
calibration_ids = { left = "left", right = "right" }
```

To exercise the interface with a
stand-in with no physics, add `--no-physics-stand-in` and use different ports from
any real station. The stand-in checks the control interface and configured geometry guards;
it has no physical contact, gravity or servo loads.

Native registered-arm setup uses a 3-second independent guard heartbeat timeout
for both stock and TPU assemblies, including agent runs. Direct hardware
profiles inherit a 0.5-second timeout by default.

## Controls

From the checkout root, use `cargo run --locked -p hebbian-rua-cli -- state`
to read the same arms. Replace `state` with `enable <arm>` or `stop <arm>`
for power or stop commands. While the viewer holds the lease, motion commands
exit 4 and report the holder. See the [CLI guide](../cli/README.md) for the
command installed by the release wheel.

Choose **Manual** while the arm is disabled, then select and enable the arm.
Enabling acquires the manual lease. If another client holds control, it must
release control first. Disabling the last enabled arm releases the lease and
clears the goal and preview. To continue in Agent mode, follow the
[viewer handoff](getting-started.md#enable-the-arm-in-agent-mode).
If the viewer loses its lease while the arm is still enabled, the goal clears.
Use **Reconnect manual control** when control is available again.
The station checks [interlocks and command requirements](control-model.md).

- **Enable arm** saves and holds the measured starting pose. **Stop and hold**
  stops motion and is available even when another client holds the control lease.
- **Rehome and disable** returns the arm to the saved pose before removing torque.
  Use it when the saved pose lets the arm rest securely and the path is clear.
  **Rehome** returns to the saved pose and keeps holding. If a saved joint angle
  is past a limit by at most the profile's recovery tolerance, rehoming uses
  the limit for that joint. A saved angle beyond that tolerance is refused.
- **Disable in place…** is below the main arm controls and removes torque at
  the current pose. Physically support the arm before confirming.
- **Use current position** copies the measured tool position and gripper
  opening into a preview. Drag the 3D arrows, or open **Motion options** to
  enter X, Y and Z in millimetres and select **Preview coordinates**.
  Previewing does not move the motors.
- **Approach** lets the solver choose an orientation, or holds a downward,
  forward or current approach. **Jaw yaw** sets the gripper rotation. The
  station reports unreachable yaw ranges and reasons when available.
- **Move to goal** sends the latest confirmed preview and then applies the
  previewed gripper opening. A constrained preview is a feasible local result,
  not a globally closest-pose guarantee. An IK arm moves to the previewed
  joints; a policy arm receives the requested pose. The ghost is a prediction,
  not proof of physical tracking or object clearance.
- **Follow goal while dragging** is opt-in for IK arms. Dragging streams
  previewed destinations through the station's motion smoother; releasing,
  losing focus or stale feedback stops following. The gripper slider sends
  coalesced updates while follow is enabled. On release, the viewer waits for
  any in-flight command before stopping the arm that received it.
- **Speed** sets the joint-speed cap for the next movement or drag. It does not
  retime a movement already accepted by the station. Learned motion uses the
  profile's joint-speed limit.
- **Clear goal** removes the goal and preview and stops following. An old
  preview response cannot authorize a move after the goal changes or clears.

The preview checks the robot display meshes against the station's configured
surface plane and leeway. A blocked preview cannot be used for a move or follow
goal, and the last valid ghost remains visible. The station enforces its own
limits for every client. Verify physical clearance from objects and other arms
before moving. See the [surface guard reference](control-invariants.md#table-surface-guard)
for configuration.

## Recovering from a fault

If an arm's bus stops answering for about half a second, the station interrupts
the command, raises the `arm_disconnected` interlock (motion is refused, and every
arm's revision moves so stale commands are refused) and retries the serial connection. It does not
deliberately turn torque off. Powered servos can continue toward their last goals
while communication is unavailable, so the interrupted command is not a success.
If servo power is lost, software cannot prevent the arm from falling.

After the bus reconnects, the station reads motor positions, torque and health.
If all motors still have torque and the checks pass, it establishes and verifies a
hold at the measured position and clears the interlock; a client continues with a
fresh observation. If a motor lost torque or a recovery check fails, the arm raises
the `recovery_approval_required` interlock and waits for an operator. Inspect and
support the arm, check that the workspace is clear, and approve recovery only when it
is safe; anyone may send `approve_recovery`. The station rechecks the arm and verifies
a hold before clearing the interlock. A lease acquired with
`on_interruption: return_lease` is returned instead of kept; see the
[control model](control-model.md).

If the station reports a fault, inspect the arm and resolve the cause first. The
viewer offers **Clear fault** when an arm is faulted. It only clears the
controller's fault after it has received recent feedback showing that all motors,
including the gripper, are healthy and disabled. Clearing does not enable or move
the arm. If feedback stays stale while the controller is still online, check the
arm's power and USB connection and inspect the reported fault.

If the motor controller stops responding or exits, the station and viewer stay
available. A separate guard can take over the serial connection and supervise the
arm's hold before releasing it. The viewer reports the controller and guard
status; torque is unknown until the motors answer again. Support every affected
arm before selecting **I have supported all affected arms** and **Reconnect arm**. The
station waits for the guard to release the connection, then checks fresh motor
feedback, faults and torque. The arm returns disabled; select **Enable arm**
separately when it is safe to do so.

If reconnect fails, inspect the displayed reason and guard report. Check the USB
connection and power; with the arm supported, power cycle it if needed and retry.
The controller's previous fault and leases do not survive reconnect. Inspect the
original fault before enabling, and acquire a new lease before sending motion.
For a station or profile change, support and disable the arm before restarting
the station as described in [station operation](reference.md#station-server).

## Troubleshooting physical tracking

If the ghost reaches a target but the physical arm does not, record the arm move's
outcome, detail, target joints and achieved joints. Compare measured joints with
the ghost and check model signs and offsets. For jerky motion or a hot motor,
stop and power down, support the arm and let it cool. Check for collision,
binding, mechanical damage and wiring or supply problems before further motion.

Check the affected servo’s actual P/I/D registers against the gains the pinned
so101 runtime defines for its joint (P=32 on shoulder lift and elbow flex, 16
elsewhere; Return_Delay_Time 0, since the factory 250 slows every read). The
driver verifies them during preflight but does **not** write them to the motor. A gain from another robot is not a
safe substitute for measuring this arm. Track temperature, load and error while
investigating, and do not relax the tracking, timing or temperature limits
to hide a problem. The controller owns the serial port while running; do not open
a second motor connection without first safely stopping the disabled controller.

## Hardware validation

Bundled limits are starting values. Their current values live in
[`profiles/so101.toml`](../profiles/so101.toml); per-arm profiles may override them.
Servo gains are defined by joint in the pinned so101 runtime.
Validate tracking, settling, bus timing, load, temperature and supply voltage on
the actual arm before increasing speed. Encoder-based forward kinematics is not
an independent measurement of Cartesian accuracy.

Station code or profile changes require supporting and disabling the arm before
restarting the station. See [station operation](reference.md#station-server).
