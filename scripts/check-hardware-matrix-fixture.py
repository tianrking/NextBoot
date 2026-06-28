#!/usr/bin/env python3
"""Exercise check-hardware-matrix.py with temporary positive and negative CSVs."""

from __future__ import annotations

import csv
import subprocess
import sys
import tempfile
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
CHECKER = PROJECT_DIR / "scripts" / "check-hardware-matrix.py"
HEADER = [
    "timestamp",
    "commit",
    "branch",
    "host_arch",
    "device",
    "media",
    "bus",
    "layout",
    "data_fs",
    "sector_size",
    "image_type",
    "firmware",
    "result",
    "report",
    "notes",
]


PASS_ROWS = [
    ("nvme-512", "fixed", "nvme", "split", "exfat", "512", "iso", "pass"),
    ("nvme-4k", "nvme", "nvme", "split", "exfat", "4096", "iso", "pass"),
    ("usb-stick", "usb", "usb", "split", "fat32", "512", "iso", "pass"),
    ("usb-ntfs-wimboot", "enclosure", "usb", "split", "ntfs", "512", "windows-wimboot", "pass"),
    ("usb-4k-vhdx", "enclosure", "usb", "split", "exfat", "4096", "vhdx", "pass"),
    ("sata-ntfs", "sata", "ahci", "split", "ntfs", "512", "iso", "pass"),
    ("sd-fat32", "sd", "sd", "split", "fat32", "512", "iso", "pass"),
    ("linux-ext4", "fixed", "nvme", "split", "ext4", "4096", "linux-plugins", "pass"),
    ("linux-xfs-vlnk", "usb", "usb", "split", "xfs", "4096", "vlnk-iso", "pass"),
    ("udf-windows", "other", "virtio", "split", "udf", "512", "windows-iso", "pass"),
]


def row(values: tuple[str, str, str, str, str, str, str, str]) -> dict[str, str]:
    device, media, bus, layout, data_fs, sector_size, image_type, result = values
    return {
        "timestamp": "2026-06-28T00:00:00Z",
        "commit": "fixture",
        "branch": "fixture",
        "host_arch": "fixture",
        "device": device,
        "media": media,
        "bus": bus,
        "layout": layout,
        "data_fs": data_fs,
        "sector_size": sector_size,
        "image_type": image_type,
        "firmware": "Fixture UEFI",
        "result": result,
        "report": f"target/hardware-reports/{device}.md",
        "notes": "fixture row",
    }


def write_csv(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=HEADER)
        writer.writeheader()
        writer.writerows(rows)


def run_checker(path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(CHECKER), "--csv", str(path)],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> int:
    try:
        target_dir = PROJECT_DIR / "target"
        target_dir.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="hardware-matrix-check-", dir=target_dir) as tmp:
            workdir = Path(tmp)
            complete_csv = workdir / "complete.csv"
            incomplete_csv = workdir / "incomplete.csv"
            rows = [row(values) for values in PASS_ROWS]
            write_csv(complete_csv, rows)
            write_csv(incomplete_csv, rows[:-1])

            complete = run_checker(complete_csv)
            require(complete.returncode == 0, complete.stdout)
            require("Covered required rows: 10" in complete.stdout, complete.stdout)
            require("Missing required rows: 0" in complete.stdout, complete.stdout)

            incomplete = run_checker(incomplete_csv)
            require(incomplete.returncode == 1, incomplete.stdout)
            require("Covered required rows: 9" in incomplete.stdout, incomplete.stdout)
            require("Missing required rows: 1" in incomplete.stdout, incomplete.stdout)
            require("UDF Windows ISO" in incomplete.stdout, incomplete.stdout)
    except AssertionError as error:
        print(f"hardware matrix fixture check failed: {error}", file=sys.stderr)
        return 1

    print("hardware matrix fixture check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
