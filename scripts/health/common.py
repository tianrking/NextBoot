"""Shared health-check types and project helpers."""

from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[2]
DEFAULT_LINE_LIMIT = 500
DEFAULT_BUILD_TARGET = "x86_64-unknown-uefi"
CHECK_EXTENSIONS = {".py", ".rs", ".sh"}
HOST_TEST_PACKAGES = ("nextboot-fs", "nextboot-config", "nextboot-image", "nextboot-linux")


@dataclass
class CheckResult:
    name: str
    ok: bool
    details: str = ""


def project_files() -> list[Path]:
    roots = (PROJECT_DIR / "crates", PROJECT_DIR / "scripts")
    files: list[Path] = []
    for root in roots:
        files.extend(
            path
            for path in root.rglob("*")
            if path.is_file() and path.suffix in CHECK_EXTENSIONS
        )
    return sorted(files)


def rel(path: Path) -> str:
    return str(path.relative_to(PROJECT_DIR))


def run_script_check(script_name: str, label: str) -> CheckResult:
    script = PROJECT_DIR / "scripts" / script_name
    result = subprocess.run(
        [str(script)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult(label, result.returncode == 0, result.stdout)
