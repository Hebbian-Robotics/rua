# Example viewer

The example Node server hosts the React viewer and agent loop. Run `pnpm build`
and `pnpm start:viewer`, then open <http://127.0.0.1:8793/viewer/>. Set
`HEBBIAN_STATION_URL` to connect to a station other than the local default. Follow the
[viewer build instructions](../../../docs/install-from-source.md#optional-viewer)
and start a station with [your arm connected](../../../docs/getting-started.md).
Use [MuJoCo](../../../docs/simulation.md) to test the viewer without hardware.
Each browser tab opens a session on the viewer backend. The backend connects
to the station through the TypeScript SDK, streams state to the tab, and
releases its manual lease when the session ends. The browser renders the robot
model, connects directly to LiveKit for live video, and decodes recordings
served through the example backend. The station owns capture and storage.

## Use the viewer

- **Workspace** shows the robot and cameras. Choose **Manual** or **Agent**
  while the arm is disabled. Enable the arm, then adjust a target in **Manual**
  or enter a task in **Agent**. See [arm controls](../../../docs/so101-manual-control.md#controls).
- **Run options** under **Agent** configures the model and cameras for that task.
- **Add remote camera** pairs a phone. See [Cameras](../../../docs/cameras.md)
  for phone and USB camera setup.
- **Recording** starts or stops capture and configures automatic recording.
  **Recordings** opens saved sessions. See [Recording](../../../docs/recording.md).
- **Settings** controls appearance, pose overlays, camera labels, and agent
  connection status.

## Develop the viewer

From the repository root, run `pnpm dev` and open the Vite viewer URL. Start the station separately. Add `--manual-control` to exercise manual motion in simulation.
Use `pnpm check` and `pnpm test:viewer` to check changes.

- [`src/app/`](src/app/) contains the page entry points and workspace. The full
  viewer entry point adds recording controls and auto-record callbacks to the
  live workspace. Playback has its own entry point.
- [`src/features/`](src/features/) contains the controls, cameras, scene,
  recordings, and settings.
- [`server/src/`](server/src/) hosts the [reference agent](../agent/README.md).
- The [TypeScript SDK](../../../typescript-client/README.md) owns station access
  and shared schemas. See the [station reference](../../../docs/reference.md#station-server)
  to build a different viewer.
