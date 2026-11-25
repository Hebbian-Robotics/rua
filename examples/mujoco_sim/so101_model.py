import re
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
ASSET_PIN_PATH = REPOSITORY_ROOT / "robots/so101-assets.toml"
ASSET_DIRECTORY = REPOSITORY_ROOT / "robots/so101"

MODEL_CACHE_DIRECTORY = REPOSITORY_ROOT / ".cache/so101-mujoco-model"


@dataclass(frozen=True)
class So101MujocoModelPin:
    repository: str
    commit: str
    directory: str
    model: str
    urdf: str
    camera_board_frame: str

    @classmethod
    def load(cls) -> "So101MujocoModelPin":
        pin = tomllib.loads(ASSET_PIN_PATH.read_text())
        if re.fullmatch(r"[0-9a-f]{40}", pin.get("revision", "")) is None:
            raise ValueError(f"{ASSET_PIN_PATH} must pin a full commit hash")
        assemblies = tomllib.loads(
            (ASSET_DIRECTORY / "robot-assemblies.toml").read_text()
        )
        assembly = assemblies["variants"]["soft-gripper-camera"]
        return cls(
            f"https://github.com/{pin['repository']}.git",
            pin["revision"],
            pin["asset_directory"],
            assembly["mjcf"],
            assembly["urdf"],
            assembly["camera_board_frame"],
        )

    @property
    def checkout_directory(self) -> Path:
        return MODEL_CACHE_DIRECTORY / self.commit

    @property
    def model_path(self) -> Path:
        return self.checkout_directory / self.directory / self.model


def run_git(*arguments: str, checkout: Path) -> None:
    subprocess.run(["git", "-C", str(checkout), *arguments], check=True)


def prepare_so101_mujoco_model(pin: So101MujocoModelPin) -> Path:
    # A sparse checkout of the pinned commit fetches only the model directory's files.
    if not pin.model_path.is_file():
        checkout = pin.checkout_directory
        checkout.mkdir(parents=True, exist_ok=True)
        if not (checkout / ".git").is_dir():
            run_git("init", "--quiet", checkout=checkout)
            run_git("remote", "add", "origin", pin.repository, checkout=checkout)
        run_git("sparse-checkout", "set", pin.directory, checkout=checkout)
        run_git(
            "fetch",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            "origin",
            pin.commit,
            checkout=checkout,
        )
        run_git("checkout", "--quiet", "--detach", pin.commit, checkout=checkout)
    return pin.model_path


def simulated_arm_model(profile_urdf: Path) -> tuple[Path, str]:
    pin = So101MujocoModelPin.load()
    simulated_urdf = ASSET_DIRECTORY / pin.urdf
    configured_urdf = REPOSITORY_ROOT / profile_urdf
    if configured_urdf.read_bytes() != simulated_urdf.read_bytes():
        raise ValueError(
            f"The MuJoCo fixture simulates {simulated_urdf}; the profile uses {profile_urdf}"
        )
    return prepare_so101_mujoco_model(pin), pin.camera_board_frame
