#!/usr/bin/env python3
"""Run lightweight structural and script health checks for NextBoot."""

from __future__ import annotations

import argparse
import os

from health.common import DEFAULT_BUILD_TARGET, DEFAULT_LINE_LIMIT, HOST_TEST_PACKAGES, CheckResult
from health.integration_checks import (
    check_flash_dry_run,
    check_hardware_matrix_fixture,
    check_hardware_report,
    check_qemu_image_matrix,
    check_qemu_matrix,
)
from health.rust_checks import check_build, check_host_tests
from health.static_checks import (
    check_line_lengths,
    check_python_compile,
    check_shell_syntax,
)


def run_checks(
    line_limit: int,
    skip_flash: bool,
    skip_qemu_matrix: bool,
    skip_qemu_image_matrix: bool,
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
    if not skip_qemu_image_matrix:
        checks.append(check_qemu_image_matrix())
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
        "--skip-qemu-image-matrix",
        action="store_true",
        help="skip no-run QEMU image generation matrix checks",
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
        help=f"skip host cargo test for {', '.join(HOST_TEST_PACKAGES)}",
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
        args.skip_qemu_image_matrix,
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
