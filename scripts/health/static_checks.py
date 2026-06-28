"""Static source and script health checks."""

from __future__ import annotations

import py_compile
import subprocess

from health.common import CheckResult, PROJECT_DIR, project_files, rel


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
