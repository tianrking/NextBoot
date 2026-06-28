#!/usr/bin/env python3
"""Run lightweight structural and script health checks for NextBoot."""

from __future__ import annotations

import argparse
import py_compile
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
DEFAULT_LINE_LIMIT = 500
CHECK_EXTENSIONS = {".py", ".rs", ".sh"}


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


def check_line_lengths(limit: int) -> CheckResult:
    offenders: list[str] = []
    for path in project_files():
        line_count = len(path.read_text(errors="replace").splitlines())
        if line_count > limit:
            offenders.append(f"{rel(path)}: {line_count} lines")

    if offenders:
        return CheckResult(
            f"source files are <= {limit} lines",
            False,
            "\n".join(offenders),
        )
    return CheckResult(f"source files are <= {limit} lines", True)


def check_python_compile() -> CheckResult:
    failures: list[str] = []
    for path in sorted((PROJECT_DIR / "scripts").rglob("*.py")):
        try:
            py_compile.compile(str(path), doraise=True)
        except py_compile.PyCompileError as error:
            failures.append(f"{rel(path)}:\n{error.msg}")

    if failures:
        return CheckResult("Python scripts compile", False, "\n\n".join(failures))
    return CheckResult("Python scripts compile", True)


def check_shell_syntax() -> CheckResult:
    shell_files = [path for path in project_files() if path.suffix == ".sh"]
    result = subprocess.run(
        ["bash", "-n", *map(str, shell_files)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult("shell scripts parse", result.returncode == 0, result.stdout)


def check_flash_dry_run() -> CheckResult:
    script = PROJECT_DIR / "scripts" / "check-flash-dry-run.py"
    result = subprocess.run(
        [str(script)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult("flash dry-run media planning", result.returncode == 0, result.stdout)


def check_qemu_matrix() -> CheckResult:
    script = PROJECT_DIR / "scripts" / "check-qemu-matrix.py"
    result = subprocess.run(
        [str(script)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult("QEMU smoke matrix declaration", result.returncode == 0, result.stdout)


def run_checks(line_limit: int, skip_flash: bool, skip_qemu_matrix: bool) -> list[CheckResult]:
    checks = [
        check_line_lengths(line_limit),
        check_python_compile(),
        check_shell_syntax(),
    ]
    if not skip_flash:
        checks.append(check_flash_dry_run())
    if not skip_qemu_matrix:
        checks.append(check_qemu_matrix())
    return checks


def print_result(result: CheckResult) -> None:
    status = "ok" if result.ok else "FAIL"
    print(f"{status} - {result.name}")
    if result.details and (not result.ok or result.details.strip()):
        print(result.details.rstrip())


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--line-limit",
        type=int,
        default=DEFAULT_LINE_LIMIT,
        help=f"maximum allowed lines per source/script file (default: {DEFAULT_LINE_LIMIT})",
    )
    parser.add_argument(
        "--skip-flash-dry-run",
        action="store_true",
        help="skip flash.sh dry-run media planning checks",
    )
    parser.add_argument(
        "--skip-qemu-matrix",
        action="store_true",
        help="skip qemu-smoke-matrix.sh coverage declaration checks",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    results = run_checks(args.line_limit, args.skip_flash_dry_run, args.skip_qemu_matrix)
    for result in results:
        print_result(result)
    return 0 if all(result.ok for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
