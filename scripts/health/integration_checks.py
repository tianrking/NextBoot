"""Health checks that delegate to focused project scripts."""

from __future__ import annotations

from health.common import CheckResult, run_script_check


def check_flash_dry_run() -> CheckResult:
    return run_script_check(
        "check-flash-dry-run.py",
        "flash dry-run media planning",
    )


def check_qemu_matrix() -> CheckResult:
    return run_script_check(
        "check-qemu-matrix.py",
        "QEMU smoke matrix declaration",
    )


def check_qemu_image_matrix() -> CheckResult:
    return run_script_check(
        "check-qemu-image-matrix.py",
        "QEMU image generation matrix",
    )


def check_hardware_report() -> CheckResult:
    return run_script_check(
        "check-hardware-report.py",
        "hardware report generation",
    )


def check_hardware_matrix_fixture() -> CheckResult:
    return run_script_check(
        "check-hardware-matrix-fixture.py",
        "hardware matrix fixture coverage",
    )


def check_secure_boot_policy() -> CheckResult:
    return run_script_check(
        "check-secure-boot-policy.py",
        "Secure Boot release policy",
    )
