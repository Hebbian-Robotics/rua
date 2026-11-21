import json
import os
import subprocess
from pathlib import Path

from tests.fixture_station import resolve_simulator_arm


def test_missing_onnx_runtime_is_reported(server_binary: Path, tmp_path: Path) -> None:
    config = resolve_simulator_arm(server_binary)
    # CONSTRAINT: The fixture must pass the file hash check to exercise runtime loading.
    model_path = tmp_path / "model.onnx"
    model_path.write_bytes(b"model bytes")
    trace_path = tmp_path / "replay.json"
    trace_path.write_text(
        json.dumps(
            {
                "urdf": str(config.urdf),
                "arm_joints": config.driven_joints[: len(config.station.joint_names)],
                "tool_link": config.tool_link,
                "onnx": str(tmp_path / "model.onnx"),
                "steps": [],
            }
        )
    )
    result = subprocess.run(
        [str(server_binary), "replay-policy", "--trace", str(trace_path)],
        env={**os.environ, "ORT_DYLIB_PATH": str(tmp_path / "missing-runtime.so")},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0
    assert "Cannot load ONNX Runtime" in result.stderr
    assert "panicked" not in result.stderr
