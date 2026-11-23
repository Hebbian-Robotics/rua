# Example viewer

Use the viewer to check a calibrated arm and its camera feeds, drive it
manually, or run an agent. The viewer uses the published TypeScript SDK from
the repository lockfile.

## Run the viewer on hardware

1. Complete [source setup](../../../docs/install-from-source.md#prepare-the-checkout)
   and [build the viewer](../../../docs/install-from-source.md#optional-viewer).
   If your arm needs calibration, follow the
   [calibration guide](../../../docs/so101-calibration.md).
2. For live video in the viewer, [build the media companion and configure LiveKit](../../../docs/cameras.md#connect-a-usb-camera).
   Plug in the wrist camera. If exactly one camera matches the arm's registered
   wrist selector, RUA picks it automatically. Use `--camera wrist=DEVICE`
   to override the selector.
3. With the arm supported and disabled, start the station from the repository
   root, then open the [station viewer](http://127.0.0.1:8792/viewer/).

   ```sh
   uv run cargo run --locked -p hebbian-robot-use-server -- serve --manual-control
   ```

   Add `--so101-config-directory /path/to/arm-configs` if your calibration
   registry is outside the [default location](../../../docs/so101-manual-control.md#set-up-each-so-101).

The viewer joins the station’s LiveKit room automatically. Check the
**Workspace** camera status if the wrist feed is missing. The viewer reports a
disconnected or ambiguous USB camera and a LiveKit reconnect separately. Use
**Workspace → Add remote camera → Create invite** to add a phone.

## Use the viewer

- **Manual** lets you preview and move to a target. Follow the
  [manual-control walkthrough](../../../docs/getting-started.md#2-verify-manual-control)
  to verify the arm before enabling it.
- **Agent** runs a task with a model. Follow the
  [agent setup](../../../docs/getting-started.md#3-run-your-first-agent), then
  open **Run options** to select a provider and camera views.
- **Recording** starts or stops capture when
  [recording is enabled](../../../docs/recording.md). **Recordings** opens saved
  sessions.
- **Settings** controls appearance, pose overlays, camera labels, and the agent
  server address.

## Develop the viewer

Keep the station running and run `pnpm dev:viewer` from the repository root.
Open the Vite URL ending in `/viewer/` for hot reload. Run `pnpm dev` to start
the agent server alongside Vite. Use `pnpm check` and `pnpm test:viewer` to
check changes.

To test without hardware, follow the [MuJoCo guide](../../../docs/simulation.md)
and add `--manual-control` to the simulated station command for manual motion.
Simulator snapshots and LiveKit video use
[different camera paths](../../../docs/simulation.md#camera-snapshots).

- [`src/app/`](src/app/) contains the page entry points and workspace. The full
  viewer entry point adds recording controls and auto-record callbacks to the
  live workspace. Playback has its own entry point.
- [`src/features/`](src/features/) contains the controls, cameras, scene,
  recordings, and settings.
- [`server/src/`](server/src/) hosts the [reference agent](../agent/README.md).
- The [TypeScript SDK](../../../typescript-client/README.md) owns station access
  and shared schemas. See the [station reference](../../../docs/reference.md#station-server)
  to build a different viewer.
