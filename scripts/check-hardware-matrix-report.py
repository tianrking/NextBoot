#!/usr/bin/env python3
"""Exercise hardware-matrix-report.py with temporary fixture matrices."""

from __future__ import annotations

import csv
import subprocess
import sys
import tempfile
from pathlib import Path

from hardware_matrix import EXPECTED_COLUMNS


PROJECT_DIR = Path(__file__).resolve().parents[1]
REPORTER = PROJECT_DIR / "scripts" / "hardware-matrix-report.py"


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
        writer = csv.DictWriter(handle, fieldnames=EXPECTED_COLUMNS)
        writer.writeheader()
        writer.writerows(rows)


def run_report(csv_path: Path, output: Path, check: bool = False) -> subprocess.CompletedProcess[str]:
    command = [str(REPORTER), "--csv", str(csv_path), "--output", str(output)]
    if check:
        command.append("--check")
    return subprocess.run(
        command,
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
        with tempfile.TemporaryDirectory(prefix="hardware-matrix-report-", dir=target_dir) as tmp:
            workdir = Path(tmp)
            complete_csv = workdir / "complete.csv"
            incomplete_csv = workdir / "incomplete.csv"
            report = workdir / "status.md"
            write_csv(complete_csv, [row(values) for values in PASS_ROWS])
            write_csv(incomplete_csv, [row(values) for values in PASS_ROWS[:-1]])

            complete = run_report(complete_csv, report)
            require(complete.returncode == 0, complete.stdout)
            text = report.read_text()
            require("| Required coverage | 10/10 (100%) |" in text, text)
            require("| Production hardware claim | ready |" in text, text)

            check = run_report(complete_csv, report, check=True)
            require(check.returncode == 0, check.stdout)

            incomplete = run_report(incomplete_csv, report)
            require(incomplete.returncode == 0, incomplete.stdout)
            text = report.read_text()
            require("| Required coverage | 9/10 (90%) |" in text, text)
            require("| Production hardware claim | blocked |" in text, text)
            require("- UDF Windows ISO" in text, text)

            report.write_text(text + "\n")
            stale = run_report(incomplete_csv, report, check=True)
            require(stale.returncode == 1, stale.stdout)
            require("stale" in stale.stdout, stale.stdout)

            synced = subprocess.run(
                [str(REPORTER), "--check"],
                cwd=PROJECT_DIR,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            require(synced.returncode == 0, synced.stdout)
    except AssertionError as error:
        print(f"hardware matrix report check failed: {error}", file=sys.stderr)
        return 1

    print("hardware matrix report check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
