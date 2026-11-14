"""Credentials and settings: the process environment, or a .env file.

Every entry point calls load_environment() first, so a key works the same way
everywhere: export it, or put it in a .env file in the directory you run from (or a
parent of it). An exported variable always wins over the file. Copy .env.example to
start.
"""

from pathlib import Path

from dotenv import find_dotenv, load_dotenv


def load_environment(path: Path | None = None) -> Path | None:
    """Load a .env file into the environment without replacing variables that are
    already set. With no path, look in the working directory and its parents (not
    next to the calling code, which for an installed package is site-packages).
    Returns the file loaded, or None when there is none."""
    found = path or find_dotenv(usecwd=True)
    if not found:
        return None
    load_dotenv(found, override=False)
    return Path(found)
