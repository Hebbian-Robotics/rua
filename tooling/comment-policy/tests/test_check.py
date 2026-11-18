from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from collections import Counter
from pathlib import Path
from tempfile import TemporaryDirectory
from types import ModuleType

from pydantic import BaseModel


class PolicyCase(BaseModel):
    name: str
    extension: str
    source: str
    findings: int
    directory: str = "."


def test_comment_paragraph_policy(tmp_path: Path) -> None:
    policy_directory = Path(__file__).resolve().parents[1]
    cases = [
        PolicyCase.model_validate(case)
        for case in json.loads(Path(__file__).with_name("cases.json").read_text())
    ]
    expected: dict[str, int] = {}
    for case in cases:
        source_path = tmp_path / case.directory / f"{case.name}.{case.extension}"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_bytes(case.source.encode())
        expected[str(source_path)] = case.findings
    result = subprocess.run(
        [
            sys.executable,
            str(policy_directory / "check.py"),
            "--json",
            str(tmp_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1, result.stderr
    report = json.loads(result.stdout)
    assert "Code is the primary documentation." in report["reason"]
    assert "WHY:" in report["reason"]
    assert not result.stderr
    actual = Counter(
        finding["file"]
        for finding in report["findings"]
        if finding["ruleId"].startswith("explanatory-comment-")
    )
    assert {path: actual[path] for path in expected} == expected
    assert set(actual) <= expected.keys()

    for finding in report["findings"]:
        Path(finding["file"]).unlink(missing_ok=True)
    result = subprocess.run(
        [sys.executable, str(policy_directory / "check.py"), str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "0 comment or documentation policy violations."
    assert not result.stderr

    source_path = tmp_path / "unclassified.py"
    source_path.write_text("# Assign the value.\nvalue = 1\n")
    result = subprocess.run(
        [sys.executable, str(policy_directory / "check.py"), str(source_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1
    assert f"{source_path}:1:" in result.stdout
    assert "Remove this comment" in result.stdout
    assert "Code is the primary documentation." in result.stderr

    result = subprocess.run(
        [sys.executable, str(policy_directory / "check.py"), "--json", str(tmp_path)],
        env={**os.environ, "PATH": str(tmp_path)},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 2
    report = json.loads(result.stdout)
    assert "ast-grep is required" in report["error"]
    assert "Code is the primary documentation." in report["reason"]
    assert report["findings"] == []
    assert not result.stderr


def _load_check_module() -> ModuleType:
    specification = importlib.util.spec_from_file_location(
        "comment_policy_check", Path(__file__).resolve().parents[1] / "check.py"
    )
    assert specification is not None
    assert specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    try:
        specification.loader.exec_module(module)
    finally:
        sys.path.pop(0)
    return module


def test_documentation_audit_flags_documentation_outside_public_interfaces(
    tmp_path: Path,
) -> None:
    policy_directory = Path(__file__).resolve().parents[1]
    cases = [
        PolicyCase.model_validate(case)
        for case in json.loads(
            Path(__file__).with_name("documentation_cases.json").read_text()
        )
    ]
    cases.extend(
        PolicyCase.model_validate(case)
        for case in json.loads(
            Path(__file__).with_name("schema_metadata_cases.json").read_text()
        )
    )
    expected: dict[str, int] = {}
    for case in cases:
        source_path = tmp_path / case.directory / f"{case.name}.{case.extension}"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_bytes(case.source.encode())
        expected[str(source_path)] = case.findings
    result = subprocess.run(
        [
            sys.executable,
            str(policy_directory / "check.py"),
            "--json",
            str(tmp_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    report = json.loads(result.stdout)
    actual = Counter(finding["file"] for finding in report["findings"])
    assert {path: actual[path] for path in expected} == expected

    assert "Schema descriptions belong in supported" in report["reason"]
    assert "public-interfaces.toml" in report["reason"]
    selected = tmp_path / "pydantic-field.py"
    selected_result = subprocess.run(
        [
            sys.executable,
            str(policy_directory / "check.py"),
            str(selected),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert selected_result.returncode == 1
    assert f"{selected}:2:" in selected_result.stdout
    assert "public interface directory" in selected_result.stdout
    assert "Do not move docstrings into" in selected_result.stderr

    with TemporaryDirectory(
        dir=policy_directory.parents[1] / "contracts"
    ) as public_directory:
        for case in cases:
            source_path = Path(public_directory) / f"{case.name}.{case.extension}"
            source_path.write_text(case.source)
        public_result = subprocess.run(
            [
                sys.executable,
                str(policy_directory / "check.py"),
                "--json",
                public_directory,
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert public_result.returncode == 0, public_result.stderr
        assert json.loads(public_result.stdout)["findings"] == []


def test_public_interface_allowlist_uses_repository_relative_paths() -> None:
    check = _load_check_module()
    public_directories = check.load_public_directories()
    root = check.REPOSITORY_ROOT
    assert check.is_in_public_interface(
        str(root / "sdk/src/lib.rs"), public_directories
    )
    assert not check.is_in_public_interface(
        str(root / "executor/src/sdk/lib.rs"), public_directories
    )
    assert not check.is_in_public_interface(
        str(root / "sdk-extras/lib.rs"), public_directories
    )
