# Example viewer

The station serves this React viewer at `/viewer/`. Follow the
[viewer build instructions](../../../docs/install-from-source.md#optional-viewer)
and start a station with [your arm connected](../../../docs/getting-started.md).
Use [MuJoCo](../../../docs/simulation.md) to test the viewer without hardware.
The viewer build installs the published TypeScript SDK from npm through the
repository lockfile.

## Use the viewer

See [arm controls](../../../docs/so101-manual-control.md#controls) for manual
operation, [the agent example](../agent/README.md) for model setup,
[the camera guide](../../../docs/cameras.md) for camera connections, and
[the recording guide](../../../docs/recording.md) for capture and playback.

## Develop the viewer

From the repository root, run `pnpm dev` and open the Vite viewer URL. Start the
station first with `HEBBIAN_VIEWER_DIRECTORY="$PWD/dist/viewer"` in its environment
so it validates the viewer configuration even during frontend development. Add `--manual-control` to exercise manual motion in simulation.
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
