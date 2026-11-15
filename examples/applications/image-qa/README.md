# Image Q&A

Show an image on a display in front of the arm, then ask a yes/no question such
as "Is this a Siberian Husky?" or "Is this mountain in Yosemite?". The arm rises
from rest to face the display. The model reads the wrist-camera frame, checks
its identification and the facts on the web, and the arm nods for yes or shakes
its head for no.

The arm holds one fixed viewing pose, so the model doesn't drive it. Each question
is a single OpenAI Responses call with the built-in `web_search` tool, and at
least one search is required.

## You need

- One SO-101 with [per-arm setup](https://github.com/Hebbian-Robotics/rua/blob/main/docs/hardware.md) done, resting on its
  table. Quitting rehomes the arm and disables it, and disabling drops it, so the
  start pose must be supported.
- A camera named `table` mounted on the wrist ([cameras](https://github.com/Hebbian-Robotics/rua/blob/main/docs/cameras.md)).
- A display in front of the arm, roughly at eye level for a raised gripper.
- `OPENAI_API_KEY` in your environment or `.env`, and a model that supports web
  search.

## Run

Start the station with your profile and camera, then the app:

```sh
uv run rua serve --profile ~/.config/rua/so101.toml --camera table=0
uv run --project examples/python examples/applications/image-qa/image_qa.py --model <openai-model>
```

The arm stays at the viewing pose between questions. Type a question and press
Enter; a blank line rehomes, disables and quits.

Each question writes the frame the model saw and its full reply, with sources
and the number of web searches, to `runs/<time>/` next to the script.

## Before the first run

- **Find your viewing pose.** `VIEW_JOINTS` puts the gripper 18 cm forward and
  31 cm up, with the camera tilted about 17 degrees above horizontal, for our desk.
  Move to it with a short script, save a frame, and adjust. Lowering `wrist_flex`
  tilts the camera up. Keep the image clear of the jaws.
- **Cap speed in the station.** Set `[motion] max_joint_speed` in your private
  profile; we used 1.0 rad/s.
- Keep a physical power cutoff within reach.

## What made the answers reliable

- **Let the exposure settle.** A frame taken the moment the arm arrived was washed
  out, and the model misidentified the dog. The script now waits 2 seconds before
  each frame.
- **Require web validation.** The model must search before answering rather than
  rely on memory. It cites what it found, for example the breed page that told an
  Alaskan Malamute from a Husky.
- **Ask for reasoning.** The reply says what the model sees and why before the
  `ANSWER:` line, so a wrong answer is easy to trace.

## Limits

The model won't identify real people from their faces. For a question like "Is
this DJ still alive?", it asks for a name instead and the arm doesn't gesture.
Put the name in the question, or show a caption with the name on the display.
Places, animals, objects and landmarks work without this.
