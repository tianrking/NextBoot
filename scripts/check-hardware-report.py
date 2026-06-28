#!/usr/bin/env python3
"""Check hardware-report.sh Markdown and CSV generation without real devices."""

from __future__ import annotations

import csv
import os
import subprocess
import sys
import tempfile
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
REPORT_SCRIPT = PROJECT_DIR / "scripts" / "hardware-report.sh"
EXPECTED_COLUMNS = [
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


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def write_smoke_log(path: Path) -> None:
    lines = [f"serial line {index:03d}" for index in range(140)]
    lines.append("NEXTBOOT_SMOKE_EFI_STARTED")
    path.write_text("\n".join(lines) + "\n")


def run_report(workdir: Path) -> tuple[Path, Path, str]:
    report = workdir / "report.md"
    matrix = workdir / "matrix.csv"
    smoke_log = workdir / "serial.log"
    write_smoke_log(smoke_log)

    env = os.environ.copy()
    env["NEXTBOOT_OSTYPE"] = "linux"
    command = [
        str(REPORT_SCRIPT),
        "--device",
        "fixture-usb-ssd",
        "--media",
        "enclosure",
        "--bus",
        "usb",
        "--layout",
        "split",
        "--data-fs",
        "ntfs",
        "--sector-size",
        "512",
        "--image-type",
        "windows-wimboot",
        "--firmware",
        "Fixture UEFI 1.0",
        "--result",
        "partial",
        "--notes",
        'quoted "note", comma',
        "--smoke-log",
        str(smoke_log),
        "--output",
        str(report),
        "--append-csv",
        str(matrix),
    ]
    result = subprocess.run(
        command,
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    require(result.returncode == 0, result.stdout)
    return report, matrix, result.stdout


def check_report(report: Path) -> None:
    text = report.read_text()
    required = (
        "# NextBoot Hardware Compatibility Report",
        "| Device | fixture-usb-ssd |",
        "| Media | enclosure |",
        "| Bus | usb |",
        "| Data filesystem | ntfs |",
        "| Sector size | 512 |",
        "| Image type | windows-wimboot |",
        "| Firmware | Fixture UEFI 1.0 |",
        '| Notes | quoted "note", comma |',
        "## Log Tail",
        "NEXTBOOT_SMOKE_EFI_STARTED",
    )
    for needle in required:
        require(needle in text, f"report missing {needle!r}")
    require("serial line 000" not in text, "report should include only the log tail")
    require("serial line 139" in text, "report tail should include recent log lines")


def check_matrix(matrix: Path, report: Path) -> None:
    with matrix.open(newline="") as handle:
        reader = csv.DictReader(handle)
        require(reader.fieldnames == EXPECTED_COLUMNS, "CSV header changed")
        rows = list(reader)

    require(len(rows) == 1, f"expected 1 CSV row, got {len(rows)}")
    row = rows[0]
    expected_values = {
        "device": "fixture-usb-ssd",
        "media": "enclosure",
        "bus": "usb",
        "layout": "split",
        "data_fs": "ntfs",
        "sector_size": "512",
        "image_type": "windows-wimboot",
        "firmware": "Fixture UEFI 1.0",
        "result": "partial",
        "report": str(report),
        "notes": 'quoted "note", comma',
    }
    for key, value in expected_values.items():
        require(row[key] == value, f"CSV {key}={row[key]!r}, expected {value!r}")


def main() -> int:
    try:
        target_dir = PROJECT_DIR / "target"
        target_dir.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="hardware-report-check-", dir=target_dir) as tmp:
            workdir = Path(tmp)
            report, matrix, output = run_report(workdir)
            check_report(report)
            check_matrix(matrix, report)
            require(str(report) in output, "script output did not mention report path")
            require(str(matrix) in output, "script output did not mention CSV path")
    except AssertionError as error:
        print(f"hardware report check failed: {error}", file=sys.stderr)
        return 1

    print("hardware report generation check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
