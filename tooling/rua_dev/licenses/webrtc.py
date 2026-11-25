import shutil
from pathlib import Path

from pydantic import BaseModel, Field


class CargoBuildMessage(BaseModel):
    reason: str
    package_id: str | None = None
    linked_paths: list[str] = Field(default_factory=list)


def collect_webrtc_license(build_messages: Path, destination: Path) -> None:
    licenses: set[Path] = set()
    for line in build_messages.read_text().splitlines():
        message = CargoBuildMessage.model_validate_json(line)
        if message.reason != "build-script-executed" or message.package_id is None:
            continue
        package = message.package_id.rsplit("#", 1)[-1].split("@", 1)[0]
        if package != "webrtc-sys":
            continue
        for link_path in message.linked_paths:
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
