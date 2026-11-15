"""Development tooling follows the SDK components available in a checkout."""

from pathlib import Path

import pytest
from rua_dev.contracts import available_sdk_targets, generate_contracts
from rua_dev.models import SdkTarget


@pytest.mark.parametrize(
    ("directories", "expected_targets"),
    [
        (["hebbian_rua/client"], {SdkTarget.PYTHON}),
        (["hebbian_rua/client", "sdk", "typescript-client"], set(SdkTarget)),
    ],
)
def test_default_sdk_targets_follow_included_components(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    directories: list[str],
    expected_targets: set[SdkTarget],
) -> None:
    for directory in directories:
        (tmp_path / directory).mkdir(parents=True)
    monkeypatch.chdir(tmp_path)
    assert available_sdk_targets() == expected_targets


@pytest.mark.parametrize("target", [SdkTarget.RUST, SdkTarget.TYPESCRIPT])
def test_missing_sdk_is_rejected_before_generating_any_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: SdkTarget
) -> None:
    output = tmp_path / "hebbian_rua/client/contract.py"
    output.parent.mkdir(parents=True)
    output.write_text("existing client contract\n")
    monkeypatch.chdir(tmp_path)
    with pytest.raises(ValueError, match="components present in this checkout"):
        generate_contracts(frozenset({SdkTarget.PYTHON, target}))
    assert output.read_text() == "existing client contract\n"
    assert sorted(
        path.relative_to(tmp_path).as_posix() for path in tmp_path.rglob("*.py")
    ) == ["hebbian_rua/client/contract.py"]
