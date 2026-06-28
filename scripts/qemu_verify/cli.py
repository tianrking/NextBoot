"""Verify NextBoot raw QEMU disk images.

This checker validates the GPT layout and the small FAT32/exFAT/ext2/3/4/NTFS/UDF/XFS/Btrfs volumes
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
    parser.add_argument(
        "--data-fs",
        default="exfat",
        choices=("btrfs", "exfat", "ext2", "ext3", "ext4", "fat32", "ntfs", "udf", "xfs"),
    )
    parser.add_argument("--efi-file", help="expected fallback EFI source file")
    parser.add_argument(
        "--efi-boot-name",
        default="BOOTX64.EFI",
        choices=("BOOTX64.EFI", "BOOTIA32.EFI", "BOOTAA64.EFI"),
        help="fallback EFI filename under /EFI/BOOT",
    )
    parser.add_argument(
        "--efi-loader",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="additional expected fallback EFI source file; repeatable",
    )
    parser.add_argument("--image", action="append", default=[], help="expected /ISO image file; repeatable")
    args = parser.parse_args(argv)

    efi_entries: list[tuple[str, str | None]] = [(args.efi_boot_name, args.efi_file)]
    seen = {args.efi_boot_name}
    for loader in args.efi_loader:
        if "=" not in loader:
            parser.error(f"--efi-loader must be NAME=PATH: {loader}")
        boot_name, source = loader.split("=", 1)
        boot_name = boot_name.upper()
        if boot_name not in ("BOOTX64.EFI", "BOOTIA32.EFI", "BOOTAA64.EFI"):
            parser.error(
                "--efi-loader NAME must be BOOTX64.EFI, BOOTIA32.EFI, or BOOTAA64.EFI"
            )
        if boot_name in seen:
            parser.error(f"duplicate EFI fallback loader: {boot_name}")
        if not source:
            parser.error(f"--efi-loader source path is empty for {boot_name}")
        efi_entries.append((boot_name, source))
        seen.add(boot_name)
    args.efi_entries = efi_entries
    return args


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        verify_layout(args)
    except VerifyError as exc:
        print(f"verify-qemu-image: {exc}", file=sys.stderr)
        return 1
    return 0
