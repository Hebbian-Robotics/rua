import os
import subprocess
from pathlib import Path


def check_wheel(wheel: Path) -> None:
    import tempfile

    with tempfile.TemporaryDirectory(prefix="rua-wheel-check-") as directory:
        environment = Path(directory) / "environment"
        subprocess.run(["uv", "venv", str(environment)], check=True)
        python = environment / "bin/python"
        subprocess.run(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(python),
                "--only-binary",
                ":all:",
                str(wheel),
            ],
            check=True,
        )
        subprocess.run(
            [str(python), "-I", str(Path(__file__).with_name("bundle_smoke.py"))],
            cwd=directory,
            env=os.environ | {"PATH": f"{environment / 'bin'}:/usr/bin:/bin"},
            check=True,
        )
