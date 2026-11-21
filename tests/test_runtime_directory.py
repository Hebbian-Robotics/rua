import os
import stat
from pathlib import Path

import pytest

from hebbian_rua.client import runtime
from hebbian_rua.client.runtime import runtime_directory


def mode_of(directory: Path) -> int:
    return stat.S_IMODE(directory.stat().st_mode)


def test_it_lives_under_the_system_runtime_directory_when_there_is_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    directory = runtime_directory()
    assert directory == tmp_path / "hebbian-robot-use"
    assert mode_of(directory) == 0o700
    assert runtime_directory() == directory


def test_a_directory_left_open_to_others_is_made_private(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    loose = tmp_path / "hebbian-robot-use"
    loose.mkdir(mode=0o777)
    loose.chmod(0o777)
    assert mode_of(runtime_directory()) == 0o700


def test_a_directory_that_belongs_to_another_user_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    (tmp_path / "hebbian-robot-use").mkdir(mode=0o700)
    real_uid = os.getuid()
    monkeypatch.setattr(runtime.os, "getuid", lambda: real_uid + 1)
    with pytest.raises(PermissionError, match="another user"):
        runtime_directory()
