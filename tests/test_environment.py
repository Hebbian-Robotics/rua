import os
from pathlib import Path

import pytest

from hebbian_rua.client.environment import load_environment

VARIABLE = "HEBBIAN_TEST_CREDENTIAL"


@pytest.fixture(autouse=True)
def clean_variable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(VARIABLE, raising=False)


def test_the_file_is_found_from_a_subdirectory_of_the_working_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / ".env").write_text(f"{VARIABLE}=from-parent\n")
    nested = tmp_path / "work" / "run"
    nested.mkdir(parents=True)
    monkeypatch.chdir(nested)
    assert load_environment() == tmp_path / ".env"
    assert os.environ[VARIABLE] == "from-parent"
