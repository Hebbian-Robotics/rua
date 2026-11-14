# Block oracle

Arrange some blocks, then ask a yes/no question such as "Are there more than two
blocks?" or "Are the blocks stacked?". The model drives the arm to inspect the
scene with the wrist camera and decides. The arm then nods for yes or shakes its
head for no.

The model chooses where to look with the robot tools. The gestures are fixed joint
moves, so the answer is always shown the same way.

## You need

- One SO-101 with [per-arm setup](../../../docs/hardware.md) done, resting on its
  table. The arm rehomes and disables between questions, and disabling drops it,
  so the start pose must be supported.
- A camera named `table` that the model can point, such as a wrist camera
  ([cameras](../../../docs/cameras.md)).
- A clear workspace, by default a mat in front of the arm. Questions only cover
  objects on it.
- `OPENAI_API_KEY` in your environment or `.env`.

## Run

Start the station with your profile and camera, then the oracle:

```sh
uv run rua serve --profile ~/.config/rua/so101.toml --camera table=0
uv run --project examples/python examples/applications/block-oracle/block_oracle.py --model <openai-model>
```

Type a question and press Enter; a blank line quits. Pass
`--workspace "the white tray left of the arm"` to describe a different area.

Each question writes the camera frames the model saw and the moves it made to
`runs/<time>/` next to the script. Open them to see why it answered as it did.

## Before the first run

- **Cap speed in the station.** The prompt asks for at most 0.6 rad/s, but a model
  can ignore that. Set `[motion] max_joint_speed` in your private profile so the
  station enforces a limit; we used 1.0 rad/s.
- **Check the presenting pose.** `PRESENT_JOINTS` was chosen on one arm to leave
  room for the nod before the wrist limit. Preview it on yours with
  `preview_pose`, or run the gestures alone, before asking questions.
- Keep a physical power cutoff within reach.

## What made the answers reliable

- **Name the workspace.** Without it, the model counted objects elsewhere on the
  desk. The prompt now limits every question to the workspace.
- **Look from the side.** From above, a stack looks like one block. The prompt asks
  for a low, side-on view before counting or judging stacking.
- **Ask for evidence.** The model states how many blocks it saw, where, and which
  are stacked before its `ANSWER:` line, so a wrong answer is easy to spot.
- **Log the frames.** The saved frames showed both failures above.
