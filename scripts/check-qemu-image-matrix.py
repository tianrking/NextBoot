#!/usr/bin/env python3
"""Generate and verify representative QEMU images without booting a VM."""

from __future__ import annotations

import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
TARGET = os.environ.get("TARGET", "x86_64-unknown-uefi")
ARCH_TAGS = {
    "x86_64-unknown-uefi": "x64",
    "i686-unknown-uefi": "ia32",
    "aarch64-unknown-uefi": "aa64",
}
ARCH_TAG = ARCH_TAGS.get(TARGET, "unknown")


@dataclass(frozen=True)
class ImageCase:
    name: str
    args: tuple[str, ...]
    expect: tuple[str, ...]
    artifact: str | None = None


CASES = (
    ImageCase(
        "NVMe 4K split exFAT smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "exfat", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat", "logical_block_size=4096"),
    ),
    ImageCase(
        "NVMe 4K split FAT32 smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "fat32", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32", "logical_block_size=4096"),
    ),
    ImageCase(
        "virtio 4K split exFAT smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "exfat", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split FAT32 smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "fat32", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split NTFS smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "ntfs", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ntfs",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split UDF smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "udf", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=udf",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split ext2 smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "ext2", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext2",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split ext3 smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "ext3", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext3",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "virtio 4K split ext4 smoke ISO",
        ("--bus", "virtio", "--layout", "split", "--data-fs", "ext4", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext4",
            "virtio-blk-pci",
            "logical_block_size=4096",
        ),
    ),
    ImageCase(
        "USB 512 split FAT32 smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "fat32", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32", "usb-storage"),
    ),
    ImageCase(
        "USB 4K split exFAT smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "exfat", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split FAT32 smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "fat32", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split NTFS smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "ntfs", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ntfs",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split UDF smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "udf", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=udf",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split ext2 smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "ext2", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext2",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split ext3 smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "ext3", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext3",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "USB 4K split ext4 smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "ext4", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext4",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "SD 512 split FAT32 smoke ISO",
        ("--bus", "sd", "--layout", "split", "--data-fs", "fat32", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32", "sd-card"),
    ),
    ImageCase(
        "SD 512 split exFAT smoke ISO",
        ("--bus", "sd", "--layout", "split", "--data-fs", "exfat", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat", "sd-card"),
    ),
    ImageCase(
        "SD 512 split NTFS smoke ISO",
        ("--bus", "sd", "--layout", "split", "--data-fs", "ntfs", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ntfs", "sd-card"),
    ),
    ImageCase(
        "SD 512 split UDF smoke ISO",
        ("--bus", "sd", "--layout", "split", "--data-fs", "udf", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=udf", "sd-card"),
    ),
    ImageCase(
        "SATA 512 split NTFS smoke ISO",
        ("--bus", "sata", "--layout", "split", "--data-fs", "ntfs", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ntfs", "ide-hd"),
    ),
    ImageCase(
        "SATA 512 split FAT32 smoke ISO",
        ("--bus", "sata", "--layout", "split", "--data-fs", "fat32", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=fat32", "ide-hd"),
    ),
    ImageCase(
        "SATA 512 split exFAT smoke ISO",
        ("--bus", "sata", "--layout", "split", "--data-fs", "exfat", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat", "ide-hd"),
    ),
    ImageCase(
        "SATA 512 split UDF smoke ISO",
        ("--bus", "sata", "--layout", "split", "--data-fs", "udf", "--sector-size", "512", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=udf", "ide-hd"),
    ),
    ImageCase(
        "NVMe 4K split UDF Windows smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "udf", "--sector-size", "4096", "--smoke-windows-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=udf", f"nextboot-smoke-{ARCH_TAG}-windows.iso"),
    ),
    ImageCase(
        "NVMe 4K split ext2 smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "ext2", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext2", f"nextboot-smoke-{ARCH_TAG}-efi.iso"),
    ),
    ImageCase(
        "NVMe 4K split ext3 smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "ext3", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext3", f"nextboot-smoke-{ARCH_TAG}-efi.iso"),
    ),
    ImageCase(
        "NVMe 4K split ext4 smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "ext4", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext4", f"nextboot-smoke-{ARCH_TAG}-efi.iso"),
    ),
    ImageCase(
        "NVMe 4K split Btrfs smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "btrfs", "--sector-size", "4096", "--smoke-efi-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=btrfs", f"nextboot-smoke-{ARCH_TAG}-efi.iso"),
    ),
    ImageCase(
        "USB 4K split Btrfs smoke ISO",
        ("--bus", "usb", "--layout", "split", "--data-fs", "btrfs", "--sector-size", "4096", "--smoke-efi-iso"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=btrfs",
            "logical_block_size=4096",
            "usb-storage",
        ),
    ),
    ImageCase(
        "NVMe 4K split ext4 Linux plugins",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "ext4", "--sector-size", "4096", "--smoke-linux-plugins"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=ext4", f"nextboot-smoke-{ARCH_TAG}-linux.iso"),
    ),
    ImageCase(
        "NVMe 4K split XFS VLNK smoke ISO",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "xfs", "--sector-size", "4096", "--smoke-vlnk-iso"),
        ("verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=xfs", "verified 1 /ISO image file(s)"),
        f"nextboot-smoke-{ARCH_TAG}-vlnk.vlnk.iso",
    ),
    ImageCase(
        "NVMe 4K split parent-backed VHDX",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "exfat", "--sector-size", "4096", "--smoke-parent-vhdx"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat",
            f"nextboot-smoke-{ARCH_TAG}-parent.vhdx",
            f"nextboot-smoke-{ARCH_TAG}-parent-base.vhdbase",
            "verified 2 /ISO image file(s)",
        ),
        f"nextboot-smoke-{ARCH_TAG}-parent.vhdx",
    ),
    ImageCase(
        "NVMe 4K split parent-backed partial VHDX",
        (
            "--bus",
            "nvme",
            "--layout",
            "split",
            "--data-fs",
            "exfat",
            "--sector-size",
            "4096",
            "--smoke-parent-partial-vhdx",
        ),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat",
            f"nextboot-smoke-{ARCH_TAG}-parent-partial.vhdx",
            f"nextboot-smoke-{ARCH_TAG}-parent-base.vhdbase",
            "verified 2 /ISO image file(s)",
        ),
        f"nextboot-smoke-{ARCH_TAG}-parent-partial.vhdx",
    ),
    ImageCase(
        "NVMe 4K split parent-backed VDI",
        ("--bus", "nvme", "--layout", "split", "--data-fs", "exfat", "--sector-size", "4096", "--smoke-parent-vdi"),
        (
            "verified split GPT layout: NEXBOOT_EFI=FAT32 NEXBOOT_DATA=exfat",
            f"nextboot-smoke-{ARCH_TAG}-parent.vdi",
            f"nextboot-smoke-{ARCH_TAG}-parent-base.vdibase",
            "verified 2 /ISO image file(s)",
        ),
        f"nextboot-smoke-{ARCH_TAG}-parent.vdi",
    ),
)


def run(command: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def build_debug(env: dict[str, str]) -> None:
    result = run([str(PROJECT_DIR / "scripts" / "build.sh"), "debug"], env)
    require(result.returncode == 0, result.stdout)


def run_case(case: ImageCase, env: dict[str, str], index: int) -> None:
    disk_image = PROJECT_DIR / "target" / f"qemu-image-matrix-{index}.img"
    artifact_tag = f"image-matrix-{index}"
    case_env = env.copy()
    case_env["NEXTBOOT_SMOKE_ARTIFACT_TAG"] = artifact_tag
    command = [
        str(PROJECT_DIR / "scripts" / "run-qemu.sh"),
        "--mode",
        "debug",
        *case.args,
        "--no-run",
        "--disk-image",
        str(disk_image),
    ]
    result = run(command, case_env)
    require(result.returncode == 0, f"{case.name} failed:\n{result.stdout}")
    for needle in case_expectations(case, artifact_tag):
        require(needle in result.stdout, f"{case.name} output missing {needle!r}\n{result.stdout}")
    require("--no-run set; image is ready for manual testing." in result.stdout, f"{case.name} did not stop at --no-run")
    if case.artifact:
        artifact = PROJECT_DIR / "target" / tagged_artifact(case.artifact, artifact_tag)
        require(artifact.exists(), f"{case.name} did not create {artifact}")


def case_expectations(case: ImageCase, artifact_tag: str) -> tuple[str, ...]:
    return tuple(tagged_artifact(needle, artifact_tag) for needle in case.expect)


def tagged_artifact(text: str, artifact_tag: str) -> str:
    return text.replace(f"nextboot-smoke-{ARCH_TAG}", f"nextboot-smoke-{ARCH_TAG}-{artifact_tag}")


def main() -> int:
    env = os.environ.copy()
    env["TARGET"] = TARGET
    try:
        require(ARCH_TAG != "unknown", f"unsupported TARGET for QEMU image matrix: {TARGET}")
        build_debug(env)
        for index, case in enumerate(CASES, start=1):
            run_case(case, env, index)
            print(f"ok - {case.name}")
    except AssertionError as error:
        print(f"QEMU image matrix check failed: {error}", file=sys.stderr)
        return 1

    print(f"checked {len(CASES)} QEMU image generation case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
