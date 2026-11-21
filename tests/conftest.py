import os
import shutil
import subprocess
from pathlib import Path

import pytest
from hebbian_robot_use_tools.shared.paths import REPOSITORY_ROOT


def pytest_itemcollected(item: pytest.Item) -> None:
    if "server_binary" in getattr(item, "fixturenames", ()) and (
        item.get_closest_marker("server") is None
    ):
        item.add_marker(pytest.mark.server)


@pytest.fixture(scope="session")
def server_binary() -> Path:
    # WHY: Validation builds once before spawning workers and passes that exact binary.
    supplied_binary = os.environ.get("HEBBIAN_ROBOT_USE_SERVER")
    if supplied_binary:
        binary_path = Path(supplied_binary).resolve()
        if not binary_path.is_file():
            pytest.fail(f"HEBBIAN_ROBOT_USE_SERVER does not exist: {binary_path}")
        return binary_path
    if shutil.which("cargo") is None:
        pytest.skip("cargo is not installed")
    subprocess.run(["cargo", "build", "--quiet"], cwd=REPOSITORY_ROOT, check=True)
    built = REPOSITORY_ROOT / "target" / "debug" / "hebbian-robot-use-server"
    os.environ["HEBBIAN_ROBOT_USE_SERVER"] = str(built)
    return built


@pytest.fixture(autouse=True)
def built_server(request: pytest.FixtureRequest) -> None:
    if request.node.get_closest_marker("server") is not None:
        request.getfixturevalue("server_binary")
