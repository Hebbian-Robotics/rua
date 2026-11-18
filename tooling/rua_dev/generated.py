import subprocess
from collections.abc import Sequence
from pathlib import Path


def check_generated_files(paths: Sequence[Path]) -> None:
    path_arguments = [str(path) for path in paths]
    subprocess.run(
        ["git", "diff", "--exit-code", "HEAD", "--", *path_arguments], check=True
    )
    # WHY: Regeneration can restore HEAD in the worktree while leaving a different
    # generated version staged for the next commit.
    subprocess.run(
        ["git", "diff", "--exit-code", "--cached", "HEAD", "--", *path_arguments],
        check=True,
    )
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "--", *path_arguments],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    if untracked:
        raise RuntimeError(f"Generated files are not committed:\n{untracked.rstrip()}")
