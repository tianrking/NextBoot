#!/usr/bin/env python3
"""Check that qemu-smoke-matrix.sh keeps the required compatibility cases."""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
MATRIX_SCRIPT = PROJECT_DIR / "scripts" / "qemu-smoke-matrix.sh"


@dataclass(frozen=True)
class MatrixRequirement:
    name: str
    tokens: tuple[str, ...]


REQUIREMENTS: tuple[MatrixRequirement, ...] = (
    MatrixRequirement(
        "default NVMe 4K split exFAT ISO boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "default USB 512 split FAT32 ISO boot",
        ("--bus usb", "--layout split", "--data-fs fat32", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "default SD 512 split FAT32 image verification",
        ("--bus sd", "--layout split", "--data-fs fat32", "--sector-size 512", "--smoke-efi-iso", "--no-run"),
    ),
    MatrixRequirement(
        "default SD 512 split exFAT image verification",
        ("--bus sd", "--layout split", "--data-fs exfat", "--sector-size 512", "--smoke-efi-iso", "--no-run"),
    ),
    MatrixRequirement(
        "default SD 512 split NTFS image verification",
        ("--bus sd", "--layout split", "--data-fs ntfs", "--sector-size 512", "--smoke-efi-iso", "--no-run"),
    ),
    MatrixRequirement(
        "default SD 512 split UDF image verification",
        ("--bus sd", "--layout split", "--data-fs udf", "--sector-size 512", "--smoke-efi-iso", "--no-run"),
    ),
    MatrixRequirement(
        "full virtio 512 single FAT32 ISO boot",
        ("--bus virtio", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split exFAT ISO boot",
        ("--bus virtio", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split FAT32 ISO boot",
        ("--bus virtio", "--layout split", "--data-fs fat32", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split NTFS ISO boot",
        ("--bus virtio", "--layout split", "--data-fs ntfs", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split UDF ISO boot",
        ("--bus virtio", "--layout split", "--data-fs udf", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split ext2 ISO boot",
        ("--bus virtio", "--layout split", "--data-fs ext2", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split ext3 ISO boot",
        ("--bus virtio", "--layout split", "--data-fs ext3", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full virtio 4K split ext4 ISO boot",
        ("--bus virtio", "--layout split", "--data-fs ext4", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split exFAT ISO boot",
        ("--bus usb", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split FAT32 ISO boot",
        ("--bus usb", "--layout split", "--data-fs fat32", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split NTFS ISO boot",
        ("--bus usb", "--layout split", "--data-fs ntfs", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split UDF ISO boot",
        ("--bus usb", "--layout split", "--data-fs udf", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split ext2 ISO boot",
        ("--bus usb", "--layout split", "--data-fs ext2", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split ext3 ISO boot",
        ("--bus usb", "--layout split", "--data-fs ext3", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full USB 4K split ext4 ISO boot",
        ("--bus usb", "--layout split", "--data-fs ext4", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full SATA 512 split NTFS ISO boot",
        ("--bus sata", "--layout split", "--data-fs ntfs", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full SATA 512 split FAT32 ISO boot",
        ("--bus sata", "--layout split", "--data-fs fat32", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full SATA 512 split exFAT ISO boot",
        ("--bus sata", "--layout split", "--data-fs exfat", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full SATA 512 split UDF ISO boot",
        ("--bus sata", "--layout split", "--data-fs udf", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split NTFS ISO boot",
        ("--bus nvme", "--layout split", "--data-fs ntfs", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split FAT32 ISO boot",
        ("--bus nvme", "--layout split", "--data-fs fat32", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split UDF ISO boot",
        ("--bus nvme", "--layout split", "--data-fs udf", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split ext2 ISO boot",
        ("--bus nvme", "--layout split", "--data-fs ext2", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split ext3 ISO boot",
        ("--bus nvme", "--layout split", "--data-fs ext3", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split ext4 ISO boot",
        ("--bus nvme", "--layout split", "--data-fs ext4", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split XFS ISO boot",
        ("--bus nvme", "--layout split", "--data-fs xfs", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split Btrfs ISO boot",
        ("--bus nvme", "--layout split", "--data-fs btrfs", "--sector-size 4096", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 512 split XFS ISO boot",
        ("--bus nvme", "--layout split", "--data-fs xfs", "--sector-size 512", "--smoke-efi-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K split XFS VLNK ISO boot",
        ("--bus nvme", "--layout split", "--data-fs xfs", "--sector-size 4096", "--smoke-vlnk-iso"),
    ),
    MatrixRequirement(
        "full NVMe 4K raw IMG boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-raw-img"),
    ),
    MatrixRequirement(
        "full NVMe 4K fixed VHD boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-vhd"),
    ),
    MatrixRequirement(
        "full NVMe 4K dynamic VHD boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-dynamic-vhd"),
    ),
    MatrixRequirement(
        "full NVMe 4K VHDX boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-vhdx"),
    ),
    MatrixRequirement(
        "full NVMe 4K sparse VHDX boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-sparse-vhdx"),
    ),
    MatrixRequirement(
        "full NVMe 4K partially-present VHDX boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-partial-vhdx"),
    ),
    MatrixRequirement(
        "full NVMe 4K parent-backed VHDX boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-parent-vhdx"),
    ),
    MatrixRequirement(
        "full NVMe 4K parent-chain VHDX boot",
        (
            "--bus nvme",
            "--layout split",
            "--data-fs exfat",
            "--sector-size 4096",
            "--smoke-parent-chain-vhdx",
            "--smoke-parent-chain-depth 4",
        ),
    ),
    MatrixRequirement(
        "full NVMe 4K missing-parent VHDX diagnostics",
        (
            "--bus nvme",
            "--layout split",
            "--data-fs exfat",
            "--sector-size 4096",
            "--smoke-missing-parent-vhdx",
        ),
    ),
    MatrixRequirement(
        "full NVMe 4K parent-backed partial VHDX boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-parent-partial-vhdx"),
    ),
    MatrixRequirement(
        "full NVMe 4K dynamic VDI boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-vdi"),
    ),
    MatrixRequirement(
        "full NVMe 4K static VDI boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-static-vdi"),
    ),
    MatrixRequirement(
        "full NVMe 4K sparse VDI boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-sparse-vdi"),
    ),
    MatrixRequirement(
        "full NVMe 4K discarded VDI boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-discarded-vdi"),
    ),
    MatrixRequirement(
        "full NVMe 4K parent-backed VDI boot",
        ("--bus nvme", "--layout split", "--data-fs exfat", "--sector-size 4096", "--smoke-parent-vdi"),
    ),
    MatrixRequirement(
        "full NVMe 4K parent-chain VDI boot",
        (
            "--bus nvme",
            "--layout split",
            "--data-fs exfat",
            "--sector-size 4096",
            "--smoke-parent-chain-vdi",
            "--smoke-parent-chain-depth 4",
        ),
    ),
    MatrixRequirement(
        "full NVMe 4K missing-parent VDI diagnostics",
        (
            "--bus nvme",
            "--layout split",
            "--data-fs exfat",
            "--sector-size 4096",
            "--smoke-missing-parent-vdi",
        ),
    ),
    MatrixRequirement(
        "full NVMe 4K ext4 Linux plugin boot",
        ("--bus nvme", "--layout split", "--data-fs ext4", "--sector-size 4096", "--smoke-linux-plugins"),
    ),
)


def normalized_blocks(text: str) -> list[str]:
    blocks: list[str] = []
    current: list[str] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line.startswith("run_case "):
            if current:
                blocks.append(" ".join(current))
            current = [line]
        elif current:
            if not line or line in {"fi", "else"} or line.startswith("if "):
                blocks.append(" ".join(current))
                current = []
            else:
                current.append(line.rstrip("\\").strip())
    if current:
        blocks.append(" ".join(current))
    return [" ".join(block.split()) for block in blocks]


def block_matches(block: str, tokens: tuple[str, ...]) -> bool:
    return all(token in block for token in tokens)


def main() -> int:
    text = MATRIX_SCRIPT.read_text()
    blocks = normalized_blocks(text)
    missing = [
        requirement.name
        for requirement in REQUIREMENTS
        if not any(block_matches(block, requirement.tokens) for block in blocks)
    ]

    if "NEXTBOOT_QEMU_SD_BOOT_SMOKE" not in text:
        missing.append("experimental SD boot smoke remains opt-in")

    if missing:
        print(f"checked {MATRIX_SCRIPT} ({len(blocks)} run_case block(s))")
        print("missing required QEMU matrix coverage:", file=sys.stderr)
        for name in missing:
            print(f"  - {name}", file=sys.stderr)
        return 1

    print(f"checked {len(REQUIREMENTS)} required QEMU matrix case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
