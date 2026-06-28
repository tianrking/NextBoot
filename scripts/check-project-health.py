#!/usr/bin/env python3
"""Run lightweight structural and script health checks for NextBoot."""

from __future__ import annotations

import argparse
import os
import py_compile
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
DEFAULT_LINE_LIMIT = 500
DEFAULT_BUILD_TARGET = "x86_64-unknown-uefi"
CHECK_EXTENSIONS = {".py", ".rs", ".sh"}
HOST_TEST_PACKAGE = "nextboot-fs"


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


def check_hardware_report() -> CheckResult:
    script = PROJECT_DIR / "scripts" / "check-hardware-report.py"
    result = subprocess.run(
        [str(script)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult("hardware report generation", result.returncode == 0, result.stdout)


def check_hardware_matrix_fixture() -> CheckResult:
    script = PROJECT_DIR / "scripts" / "check-hardware-matrix-fixture.py"
    result = subprocess.run(
        [str(script)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult("hardware matrix fixture coverage", result.returncode == 0, result.stdout)


def rust_toolchain_channel() -> str | None:
    toolchain = PROJECT_DIR / "rust-toolchain.toml"
    if not toolchain.exists():
        return None
    for line in toolchain.read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("channel") and '"' in stripped:
            return stripped.split('"', 2)[1]
    return None


def fallback_toolchain_bin(binary: str) -> Path | None:
    channel = rust_toolchain_channel()
    if not channel:
        return None
    toolchains = Path.home() / ".rustup" / "toolchains"
    for directory in sorted(toolchains.glob(f"{channel}*")):
        candidate = directory / "bin" / binary
        if candidate.exists() and os.access(candidate, os.X_OK):
            return candidate
    return None


def usable_binary(path: str | Path, args: list[str]) -> bool:
    try:
        result = subprocess.run(
            [str(path), *args],
            cwd=PROJECT_DIR,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return False
    return result.returncode == 0


def resolve_rustc() -> Path | None:
    env_rustc = os.environ.get("RUSTC")
    if env_rustc and usable_binary(env_rustc, ["--print", "sysroot"]):
        return Path(env_rustc)
    return fallback_toolchain_bin("rustc")


def resolve_cargo(rustc: Path) -> Path | None:
    env_cargo = os.environ.get("CARGO")
    if env_cargo and usable_binary(env_cargo, ["--version"]):
        return Path(env_cargo)
    sibling = rustc.parent / "cargo"
    if sibling.exists() and os.access(sibling, os.X_OK):
        return sibling
    return fallback_toolchain_bin("cargo")


def rustc_host_target(rustc: Path) -> str | None:
    result = subprocess.run(
        [str(rustc), "-vV"],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if result.returncode != 0:
        return None
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.split(":", 1)[1].strip()
    return None


def check_host_tests() -> CheckResult:
    rustc = resolve_rustc()
    if rustc is None:
        return CheckResult("Rust host unit tests", False, "could not resolve rustc")
    cargo = resolve_cargo(rustc)
    if cargo is None:
        return CheckResult("Rust host unit tests", False, "could not resolve cargo")
    host_target = rustc_host_target(rustc)
    if not host_target:
        return CheckResult("Rust host unit tests", False, "could not resolve rustc host target")

    env = os.environ.copy()
    env["RUSTC"] = str(rustc)
    result = subprocess.run(
        [
            str(cargo),
            "test",
            "-p",
            HOST_TEST_PACKAGE,
            "--lib",
            "--target",
            host_target,
        ],
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    name = f"Rust host unit tests ({HOST_TEST_PACKAGE}, {host_target})"
    return CheckResult(name, result.returncode == 0, result.stdout)


def check_build(build_target: str) -> CheckResult:
    env = os.environ.copy()
    env["TARGET"] = build_target
    result = subprocess.run(
        [str(PROJECT_DIR / "scripts" / "build.sh"), "check"],
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult(f"UEFI build check ({build_target})", result.returncode == 0, result.stdout)


def run_checks(
    line_limit: int,
    skip_flash: bool,
    skip_qemu_matrix: bool,
    skip_hardware_report: bool,
    skip_hardware_matrix_fixture: bool,
    skip_host_tests: bool,
    skip_build: bool,
    build_target: str,
) -> list[CheckResult]:
    checks = [
        check_line_lengths(line_limit),
        check_python_compile(),
        check_shell_syntax(),
    ]
    if not skip_flash:
        checks.append(check_flash_dry_run())
    if not skip_qemu_matrix:
        checks.append(check_qemu_matrix())
    if not skip_hardware_report:
        checks.append(check_hardware_report())
    if not skip_hardware_matrix_fixture:
        checks.append(check_hardware_matrix_fixture())
    if not skip_host_tests:
        checks.append(check_host_tests())
    if not skip_build:
        checks.append(check_build(build_target))
    return checks


def print_result(result: CheckResult, verbose: bool) -> None:
    status = "ok" if result.ok else "FAIL"
    print(f"{status} - {result.name}")
    if result.details and (verbose or not result.ok):
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
    parser.add_argument(
        "--skip-build-check",
        action="store_true",
        help="skip scripts/build.sh check",
    )
    parser.add_argument(
        "--skip-hardware-report",
        action="store_true",
        help="skip hardware-report.sh Markdown/CSV generation checks",
    )
    parser.add_argument(
        "--skip-hardware-matrix-fixture",
        action="store_true",
        help="skip temporary CSV fixture checks for check-hardware-matrix.py",
    )
    parser.add_argument(
        "--skip-host-tests",
        action="store_true",
        help=f"skip host cargo test for {HOST_TEST_PACKAGE}",
    )
    parser.add_argument(
        "--build-target",
        default=os.environ.get("TARGET", DEFAULT_BUILD_TARGET),
        help=f"UEFI target for scripts/build.sh check (default: TARGET or {DEFAULT_BUILD_TARGET})",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="print successful subcommand output as well as failures",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    results = run_checks(
        args.line_limit,
        args.skip_flash_dry_run,
        args.skip_qemu_matrix,
        args.skip_hardware_report,
        args.skip_hardware_matrix_fixture,
        args.skip_host_tests,
        args.skip_build_check,
        args.build_target,
    )
    for result in results:
        print_result(result, args.verbose)
    return 0 if all(result.ok for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
