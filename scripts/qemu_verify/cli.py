"""Verify NextBoot raw QEMU disk images.

This checker validates the GPT layout and the small FAT32/exFAT/NTFS volumes
created by scripts/run-qemu.sh. It intentionally focuses on the structures the
bootloader depends on: partition discovery, filesystem detection, directory
enumeration, file sizes, and physical file extents.
"""

from __future__ import annotations

import argparse
import sys

from .common import VerifyError
from .verify import verify_layout

def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disk-image", required=True, help="raw disk image to verify")
    parser.add_argument("--sector-size", required=True, type=int, choices=(512, 4096))
    parser.add_argument("--layout", required=True, choices=("single", "split"))
    parser.add_argument("--data-fs", default="exfat", choices=("exfat", "fat32", "ntfs"))
    parser.add_argument("--efi-file", help="expected BOOTX64.EFI source file")
    parser.add_argument("--image", action="append", default=[], help="expected /ISO image file; repeatable")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        verify_layout(args)
    except VerifyError as exc:
        print(f"verify-qemu-image: {exc}", file=sys.stderr)
        return 1
    return 0
