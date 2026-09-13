"""Portable subprocess command helpers for repository validation scripts."""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path


WINDOWS_PATH = re.compile(r"^[A-Za-z]:[\\/]")


def shell_argument(value: str) -> str:
    """Translate Windows absolute paths before passing them through Git Bash."""
    if os.name != "nt":
        return value
    prefix, separator, candidate = value.partition("=")
    if not WINDOWS_PATH.match(candidate if separator else value):
        return value
    path = candidate if separator else value
    converted = f"/{path[0].lower()}{path[2:]}".replace("\\", "/")
    return f"{prefix}{separator}{converted}" if separator else converted


def shell_command(*command: str) -> list[str]:
    """Return a command that executes a shell script on every supported host."""
    if os.name != "nt":
        return list(command)
    bash = os.environ.get("NEXTBOOT_BASH")
    if not bash:
        bash = str(Path(os.environ.get("ProgramFiles", r"C:\\Program Files")) / "Git" / "bin" / "bash.exe")
    return [bash, *(shell_argument(value) for value in command)]


def shell_environment(environment: dict[str, str] | None = None) -> dict[str, str]:
    """Supply the active Python interpreter to Bash-based validation on Windows."""
    result = dict(os.environ if environment is None else environment)
    if os.name == "nt" and not result.get("PYTHON"):
        result["PYTHON"] = shell_argument(sys.executable)
    return result


def python_command(*command: str) -> list[str]:
    """Run a repository Python script with the interpreter running this check."""
    return [sys.executable, *command]
