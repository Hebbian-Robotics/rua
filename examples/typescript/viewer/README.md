# Example viewer

The example Node server hosts the React viewer and agent loop. Set at least
one of `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` in your
environment or the repository’s `.env`. Run `pnpm build` and
`pnpm start:viewer`, then open <http://127.0.0.1:8793/viewer/>. Set
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

See [arm controls](../../../docs/so101-manual-control.md#controls) for manual
operation and [the agent example](../agent/README.md) for model setup. Open
**Camera setup** in **Workspace** to connect a local or remote camera. See
[the camera guide](../../../docs/cameras.md) for connection details and
[the recording guide](../../../docs/recording.md) for capture and playback.

## Start without an agent

For manual-control testing without a model-provider key, run
`pnpm start:viewer --manual-only`. Start the station with `--manual-control`
to enable manual pose controls. The viewer opens in **Manual** and disables
**Agent** for the session.

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
