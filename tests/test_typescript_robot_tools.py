import subprocess
from pathlib import Path

import pytest

from tests.fixture_station import running_station


@pytest.mark.server
@pytest.mark.typescript
def test_typescript_robot_tools_use_the_simulated_station() -> None:
    repository_directory = Path(__file__).resolve().parent.parent
    with running_station() as station:
        subprocess.run(
            [
                "pnpm",
                "exec",
                "tsx",
                "--conditions=development",
                "typescript-client/tests/simulated-robot-tools.ts",
                station.server,
            ],
            cwd=repository_directory,
            check=True,
            timeout=45,
        )
