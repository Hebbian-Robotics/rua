# Develop the example viewer

The example viewer is a React application served as static files by the robot
station. It uses the station's HTTP API and event stream; it is not required by
other clients. For a runnable station without hardware, follow the
[simulation guide](../../../docs/simulation.md).

## Run and build

From the repository root, install the viewer dependencies with
`pnpm install --frozen-lockfile`. Start a station on `127.0.0.1:8792`, then run
`pnpm dev` and open the `/viewer/` URL printed by Vite. Vite proxies the
station's API, event stream and robot assets. Use a simulated station when
changing control behavior.

`pnpm dev` also starts the agent server (`examples/typescript/viewer/server/`, Hono on
`127.0.0.1:8793`) that runs the TypeScript loop in-process and starts, stops and lists agent runs; `pnpm dev:viewer`
and `pnpm dev:agent` run either alone, and `pnpm start:agent` runs the server
without watching. The viewer calls it directly (CORS) and works without it. The
run API's zod schemas are in `examples/typescript/viewer/shared/run-api.ts`. See
[write your own loop](../../../docs/write-your-own-loop.md#run-your-program-from-the-viewer).

For a station on a different port, start the agent server with the same station
URL; its origin is allowed automatically:

```sh
HEBBIAN_STATION_URL=http://127.0.0.1:8795 pnpm start:agent
```

The agent loads model keys from the environment or the repository's `.env` file.
If `AGENT_ALLOWED_ORIGINS` is set, it replaces the defaults: include the station
origin and any development viewer origins in its comma-separated list.

Run `pnpm format`, `pnpm check` and `pnpm test:viewer` for viewer changes.
`pnpm build` writes the station viewer to `dist/viewer/`; refresh the browser
after rebuilding. The client, agent server and tests enable the `development` export condition
and import the client source directly, so no separate client rebuild is needed. `pnpm dev:agent`
watches imported client changes; restart `pnpm start:agent` after editing source.
See the [rebuild guide](../../../docs/install-from-source.md#updating-a-checkout)
for Rust, Python and TypeScript commands.
The phone camera app is developed and deployed separately in
[Hebbian-Robotics/livekit-cameras](https://github.com/Hebbian-Robotics/livekit-cameras).
The viewer displays an invite QR code; it does not bundle the phone app.
The invite contains a short-lived camera token in its URL fragment. Only use a
phone page hosted by someone you trust.

## Code layout

- `examples/typescript/viewer/src/app/` contains entry points and the workspace shell.
- `features/` groups the control panels, cameras, robot scene, recordings,
  run details, settings and telemetry. Shared visual tokens are in
  `app/workspace.css`; playback adds its own styles. The phone app lives upstream.
- `components/ui/` contains [shadcn/ui Base UI components](https://ui.shadcn.com/docs/components/base/sidebar)
  used by both viewer pages. From the repository root, run
  `pnpm exec shadcn add <component>` to add a component. The root
  `components.json` selects Base UI Nova and the viewer's paths. Use
  `--dry-run` to preview changes to existing components and identify new
  dependencies to install with `pnpm add -w`. The sidebar lives in
  `shared/navigation.tsx`.
- `components/assistant-ui/` contains source components from the
  [assistant-ui project](https://github.com/assistant-ui/assistant-ui), used for
  the agent plan and tool calls. Their Tailwind utilities use the viewer theme
  through `app/ui.css`; update the copied sources when adopting an
  upstream fix.
- `station-connection.ts` owns the live station event stream and HTTP requests. Its listeners
  return cleanup functions. A manual lease is renewed while the connection is healthy;
  an unrenewed lease expires and the station stops its motion. Viewing a control
  panel does not take it.
- `generated/` contains schemas generated from the station's contracts. Parse
  station responses at the boundary; see `pnpm check:contracts` when changing
  those contracts.
- `features/settings/preferences.ts` defines the versioned browser preferences.
  The live workspace and recordings share overlay and theme settings. New
  preferences need defaults and a migration or fallback for stored data.
- `features/robot-scene/` owns the React Three Fiber scene and loads the SO-101
  URDF and meshes. The projection is a preview; the station remains responsible
  for command outcomes and limits. Wait for the scene's validation of the
  current projection before allowing a move or follow goal.

Keep fast-changing robot and video data close to the components that display it.
Clean up subscriptions, timers, tracks, graphics resources and object URLs when
components unmount. Keep control actions explicit and preserve the station's
reported pending, refused, fault and recovery states.

The run panel shows the current plan and notes above a chronological log of
agent reasoning summaries and tool calls. Expand a tool call to see its input
and result, or open it in the Trace tab to inspect its timing and attributes.
The trace contains instrumented spans; it is not a complete log of manual
control actions.

## Use a different viewer

A custom viewer can use the station's APIs without importing the React example.
See the [station interface reference](../../../docs/reference.md#station-server)
for the HTTP endpoints, event stream and generated contracts. Build your
own static assets, then set `HEBBIAN_VIEWER_DIRECTORY` to their directory before
starting the station. The station serves that directory at `/viewer/`; make its
asset paths work under that prefix. The separate phone camera page is optional.

The task input saves its draft in localStorage for this browser and site origin.
It survives reloads and browser restarts; clearing the input saves an empty draft.
Restoring a draft does not start a run.
