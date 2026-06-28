#!/usr/bin/env python3
"""Check update-media.sh dry-run behavior preserves the data partition."""

from __future__ import annotations

import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
UPDATE_SCRIPT = PROJECT_DIR / "scripts" / "update-media.sh"
DEFAULT_TARGETS = (
    "x86_64-unknown-uefi",
    "i686-unknown-uefi",
    "aarch64-unknown-uefi",
)


@dataclass(frozen=True)
class UpdateDryRunCase:
    name: str
    ostype: str
    args: tuple[str, ...]
    expect: tuple[str, ...]
    reject: tuple[str, ...]


CASES: tuple[UpdateDryRunCase, ...] = (
    UpdateDryRunCase(
        "macOS updates ESP only",
        "darwin",
        ("--target", "all", "/dev/rdisk9"),
        (
            "Target device: /dev/disk9",
            "BOOTX64.EFI",
            "BOOTIA32.EFI",
            "BOOTAA64.EFI",
            "+ diskutil mount /dev/disk9s1",
            "+ mkdir -p /Volumes/NEXBOOT/EFI/BOOT",
            "+ diskutil unmount /dev/disk9s1",
            "NEXTDATA would be preserved",
        ),
        (
            "partitionDisk",
            "mkfs",
            "/Volumes/NEXTDATA/ISO",
        ),
    ),
    UpdateDryRunCase(
        "Linux updates ESP only",
        "linux",
        ("--target", "x86_64-unknown-uefi", "/dev/sdz"),
        (
            "Target device: /dev/sdz",
            "BOOTX64.EFI",
            "+ sudo mount /dev/sdz1 /tmp/nextboot_update_esp",
            "+ sudo mkdir -p /tmp/nextboot_update_esp/EFI/BOOT",
            "+ sudo umount /tmp/nextboot_update_esp",
            "NEXTDATA would be preserved",
        ),
        (
            "parted",
            "mkfs",
            "/tmp/nextboot_flash_data/ISO",
        ),
    ),
)


def ensure_placeholder_efi(target: str) -> None:
    path = PROJECT_DIR / "target" / target / "debug" / "nextboot-boot.efi"
    if path.exists():
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"NEXTBOOT-UPDATE-DRY-RUN-PLACEHOLDER\n")


def run_case(case: UpdateDryRunCase) -> tuple[bool, str]:
    env = os.environ.copy()
    env["NEXTBOOT_OSTYPE"] = case.ostype
    command = (
        str(UPDATE_SCRIPT),
        "--dry-run",
        *case.args,
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

    print(f"checked {len(CASES)} update-media dry-run case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
