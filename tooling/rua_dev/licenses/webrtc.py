import json
import shutil
from pathlib import Path


def collect_webrtc_license(build_messages: Path, destination: Path) -> None:
    licenses: set[Path] = set()
    for line in build_messages.read_text().splitlines():
        message = json.loads(line)
        package = message.get("package_id", "").rsplit("#", 1)[-1].split("@", 1)[0]
        if message.get("reason") != "build-script-executed" or package != "webrtc-sys":
            continue
        for link_path in message.get("linked_paths", []):
            library_directory = Path(link_path.removeprefix("native="))
            if any(
                (library_directory / library).is_file()
                for library in ("libwebrtc.a", "webrtc.lib")
            ):
                licenses.add(library_directory.parent / "LICENSE.md")
    if len(licenses) != 1:
        raise RuntimeError("Could not identify the linked WebRTC archive's license")
    license_path = licenses.pop()
    if not license_path.is_file():
        raise RuntimeError(f"The linked WebRTC archive has no license: {license_path}")
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(license_path, destination / "LICENSE.md")
