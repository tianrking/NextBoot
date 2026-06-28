#!/usr/bin/env python3
"""Check flash.sh dry-run commands for fixed, removable, and SD-style media."""

from __future__ import annotations

import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
FLASH_SCRIPT = PROJECT_DIR / "scripts" / "flash.sh"
DEFAULT_TARGETS = (
    "x86_64-unknown-uefi",
    "i686-unknown-uefi",
    "aarch64-unknown-uefi",
)


@dataclass(frozen=True)
class FlashDryRunCase:
    name: str
    ostype: str
    args: tuple[str, ...]
    expect: tuple[str, ...]
    reject: tuple[str, ...] = ()
    image_fixture: bool = False


CASES: tuple[FlashDryRunCase, ...] = (
    FlashDryRunCase(
        "macOS rdisk split XFS placeholder and formatter",
        "darwin",
        ("--layout", "split", "--data-fs", "xfs", "/dev/rdisk99"),
        (
            "+ diskutil unmountDisk /dev/disk99",
            "+ sudo diskutil partitionDisk /dev/disk99 GPT FAT32 NEXBOOT 260MiB ExFAT NEXTDATA R",
            "+ diskutil unmount /dev/disk99s2",
            "+ sudo mkfs.xfs -f -L NEXTDATA /dev/disk99s2",
        ),
    ),
    FlashDryRunCase(
        "Linux NVMe split ext4 partition suffixes",
        "linux",
        ("--layout", "split", "--data-fs", "ext4", "/dev/nvme9n1"),
        (
            "+ sudo parted -s /dev/nvme9n1 mkpart NEXTDATA ext4 260MiB 100%",
            "+ sudo mkfs.vfat -F 32 -n NEXBOOT /dev/nvme9n1p1",
            "+ sudo mkfs.ext4 -F -L NEXTDATA /dev/nvme9n1p2",
        ),
    ),
    FlashDryRunCase(
        "Linux USB split NTFS plain partition suffixes",
        "linux",
        ("--layout", "split", "--data-fs", "ntfs", "/dev/sdz"),
        (
            "+ sudo parted -s /dev/sdz mkpart NEXTDATA ntfs 260MiB 100%",
            "+ sudo mkfs.vfat -F 32 -n NEXBOOT /dev/sdz1",
            "+ sudo mkfs.ntfs -Q -F -L NEXTDATA /dev/sdz2",
        ),
    ),
    FlashDryRunCase(
        "Linux SD/MMC single layout partition suffix",
        "linux",
        ("--layout", "single", "/dev/mmcblk9"),
        (
            "+ sudo parted -s /dev/mmcblk9 mkpart NEXBOOT fat32 1MiB 100%",
            "+ sudo mkfs.vfat -F 32 -n NEXBOOT /dev/mmcblk9p1",
        ),
        ("NEXTDATA",),
    ),
    FlashDryRunCase(
        "Multi-architecture ESP installs all fallback loaders",
        "linux",
        ("--target", "all", "--layout", "split", "--data-fs", "exfat", "/dev/sdz"),
        (
            "EFI x86_64-unknown-uefi:",
            "EFI i686-unknown-uefi:",
            "EFI aarch64-unknown-uefi:",
            "BOOTX64.EFI",
            "BOOTIA32.EFI",
            "BOOTAA64.EFI",
        ),
    ),
    FlashDryRunCase(
        "Linux split exFAT copies requested image into Data ISO directory",
        "linux",
        ("--layout", "split", "--data-fs", "exfat", "--image", "{image}", "/dev/sdz"),
        (
            "Install image:",
            "core-smoke.iso -> /ISO/core-smoke.iso",
            "+ sudo mkdir -p /tmp/nextboot_flash_data/ISO",
            "+ sudo cp",
            "/tmp/nextboot_flash_data/ISO/core-smoke.iso",
        ),
        image_fixture=True,
    ),
)


def ensure_placeholder_efi(target: str) -> None:
    path = PROJECT_DIR / "target" / target / "debug" / "nextboot-boot.efi"
    if path.exists():
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"NEXTBOOT-DRY-RUN-PLACEHOLDER\n")


def expand_args(case: FlashDryRunCase) -> tuple[str, ...]:
    if not case.image_fixture:
        return case.args
    fixture = PROJECT_DIR / "target" / "flash-dry-run" / "core-smoke.iso"
    fixture.parent.mkdir(parents=True, exist_ok=True)
    fixture.write_bytes(b"NEXTBOOT-IMAGE-COPY-FIXTURE\n")
    return tuple(str(fixture) if arg == "{image}" else arg for arg in case.args)


def run_case(case: FlashDryRunCase) -> tuple[bool, str]:
    env = os.environ.copy()
    env["NEXTBOOT_OSTYPE"] = case.ostype
    command = (
        str(FLASH_SCRIPT),
        "--dry-run",
        "--no-ventoy-assets",
        *expand_args(case),
    )
    result = subprocess.run(
        command,
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    output = result.stdout
    missing = [needle for needle in case.expect if needle not in output]
    unexpected = [needle for needle in case.reject if needle in output]
    if result.returncode != 0 or missing or unexpected:
        details = [f"case: {case.name}", f"exit: {result.returncode}"]
        if missing:
            details.append("missing:\n  - " + "\n  - ".join(missing))
        if unexpected:
            details.append("unexpected:\n  - " + "\n  - ".join(unexpected))
        details.append("output:\n" + output)
        return False, "\n".join(details)
    return True, case.name


def main() -> int:
    for target in DEFAULT_TARGETS:
        ensure_placeholder_efi(target)

    failures: list[str] = []
    for case in CASES:
        ok, message = run_case(case)
        if ok:
            print(f"ok - {message}")
        else:
            failures.append(message)

    if failures:
        print("\n\n".join(failures), file=sys.stderr)
        return 1

    print(f"checked {len(CASES)} flash dry-run case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
