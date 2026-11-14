"""Open a live camera window in the browser once the station's media service is up.

`rua serve` starts this in the background so whoever is watching the robot can see
what its cameras see. Run it yourself with `python -m hebbian_rua.live_view
[media-url]`. The browser fetches each camera's newest JPEG from the media service
directly, so nothing here keeps running once the page is open.
"""

import json
import sys
import tempfile
import time
import urllib.request
import webbrowser
from pathlib import Path

DEFAULT_MEDIA_URL = "http://127.0.0.1:8781"
# A simulator may connect its cameras well after the station starts.
CAMERA_WAIT_SECONDS = 120.0

PAGE = """<!doctype html>
<meta charset="utf-8">
<title>RUA live view</title>
<style>
  body { margin: 0; background: #111; color: #ddd; font: 14px system-ui, sans-serif; }
  main { display: flex; flex-wrap: wrap; gap: 8px; padding: 8px; }
  figure { margin: 0; flex: 1 1 480px; }
  img { width: 100%; background: #000; display: block; }
  figcaption { padding: 4px 0; }
</style>
<main></main>
<script>
const MEDIA = __MEDIA__;
const CAMERAS = __CAMERAS__;
const FRAME_INTERVAL_MS = 33;  // ~30 fps; the media service always serves its newest frame
for (const name of CAMERAS) {
  const figure = document.createElement("figure");
  const image = document.createElement("img");
  const caption = document.createElement("figcaption");
  figure.append(image, caption);
  document.querySelector("main").append(figure);
  let frames = 0;
  const next = () => {
    image.src = `${MEDIA}/cameras/${encodeURIComponent(name)}?t=${Date.now()}`;
  };
  image.onload = () => { frames += 1; setTimeout(next, FRAME_INTERVAL_MS); };
  // The station may be restarting; keep the window and retry.
  image.onerror = () => { caption.textContent = `${name}: waiting for the station`; setTimeout(next, 500); };
  setInterval(() => {
    if (frames > 0) caption.textContent = `${name}: ${frames} fps`;
    frames = 0;
  }, 1000);
  next();
}
</script>
"""


def available_cameras(media_url: str) -> list[str]:
    """The cameras the media service can serve now; empty while it is starting."""
    try:
        with urllib.request.urlopen(f"{media_url}/cameras", timeout=2) as response:
            listing = json.load(response)
    except (OSError, ValueError):
        return []
    return [camera["name"] for camera in listing if camera.get("available")]


def open_live_view(
    media_url: str = DEFAULT_MEDIA_URL, wait_seconds: float = CAMERA_WAIT_SECONDS
) -> bool:
    """Wait for at least one camera, then open the live page; False if none appeared."""
    deadline = time.monotonic() + wait_seconds
    while not (cameras := available_cameras(media_url)):
        if time.monotonic() > deadline:
            return False
        time.sleep(0.5)
    page = Path(tempfile.gettempdir()) / "rua-live-view.html"
    # Values go into the page itself: macOS drops query strings from file URLs.
    page.write_text(
        PAGE.replace("__MEDIA__", json.dumps(media_url)).replace(
            "__CAMERAS__", json.dumps(cameras)
        )
    )
    return webbrowser.open(page.as_uri())


def media_url_from_serve_arguments(arguments: list[str]) -> str:
    """The media URL for `rua serve`'s `--media-address`, reachable from this machine."""
    address = "127.0.0.1:8781"
    for index, argument in enumerate(arguments):
        if argument == "--media-address" and index + 1 < len(arguments):
            address = arguments[index + 1]
        elif argument.startswith("--media-address="):
            address = argument.split("=", 1)[1]
    host, _, port = address.rpartition(":")
    if host in ("0.0.0.0", "[::]", ""):  # bound to every interface
        host = "127.0.0.1"
    return f"http://{host}:{port}"


def main() -> None:
    media_url = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_MEDIA_URL
    page = Path(tempfile.gettempdir()) / "rua-live-view.html"
    if open_live_view(media_url):
        print(f"rua live view: opened {page.as_uri()}", file=sys.stderr)
    else:
        print(
            f"rua live view: no camera at {media_url} or no browser; open {page.as_uri()}",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main()
