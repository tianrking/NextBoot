#!/usr/bin/env python3
"""Check whether the real-hardware compatibility matrix covers required rows."""

from __future__ import annotations

import argparse
import csv
import sys
from dataclasses import dataclass
from pathlib import Path


DEFAULT_CSV = Path("docs/hardware/hardware-matrix.csv")


@dataclass(frozen=True)
class Requirement:
    name: str
    media: tuple[str, ...]
    bus: tuple[str, ...]
    sector_size: tuple[str, ...]
    data_fs: tuple[str, ...]
    image_type: tuple[str, ...]


REQUIREMENTS: tuple[Requirement, ...] = (
    Requirement("Internal SSD NVMe 512 exFAT ISO", ("fixed", "nvme"), ("nvme",), ("512",), ("exfat",), ("iso",)),
    Requirement("Internal SSD NVMe 4096 exFAT ISO", ("fixed", "nvme"), ("nvme",), ("4096",), ("exfat",), ("iso",)),
    Requirement("USB stick FAT32 ISO", ("usb",), ("usb",), ("512",), ("fat32",), ("iso",)),
    Requirement("USB SSD enclosure NTFS Windows WIMBOOT", ("usb", "enclosure"), ("usb",), ("512",), ("ntfs",), ("windows", "wimboot")),
    Requirement("USB SSD enclosure 4096 exFAT VHDX", ("usb", "enclosure"), ("usb",), ("4096",), ("exfat",), ("vhdx",)),
    Requirement("SATA SSD NTFS ISO", ("fixed", "sata"), ("sata", "ahci"), ("512",), ("ntfs",), ("iso",)),
    Requirement("SD reader FAT32 ISO", ("sd",), ("sd",), ("512",), ("fat32",), ("iso",)),
    Requirement("Linux-prepared ext4 plugins", ("fixed", "nvme", "usb", "enclosure"), ("nvme", "usb"), ("4096",), ("ext4",), ("linux", "plugins")),
    Requirement("Linux-prepared XFS VLNK ISO", ("fixed", "nvme", "usb", "enclosure"), ("nvme", "usb"), ("4096",), ("xfs",), ("vlnk", "iso")),
    Requirement("UDF Windows ISO", ("fixed", "nvme", "sata", "usb", "sd", "enclosure", "other"), ("nvme", "sata", "ahci", "usb", "sd", "virtio", "other"), ("512", "4096"), ("udf",), ("windows", "iso")),
)


def normalize(value: str | None) -> str:
    return (value or "").strip().lower().replace("_", "-")


def load_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        return list(reader)


def value_matches(row: dict[str, str], field: str, accepted: tuple[str, ...]) -> bool:
    value = normalize(row.get(field))
    return "any" in accepted or value in accepted


def image_matches(row: dict[str, str], required_tokens: tuple[str, ...]) -> bool:
    value = normalize(row.get("image_type"))
    if value == "mixed":
        return True
    return all(token in value for token in required_tokens)


def result_matches(row: dict[str, str], allow_partial: bool) -> bool:
    accepted = {"pass"}
    if allow_partial:
        accepted.add("partial")
    return normalize(row.get("result")) in accepted


def row_matches(requirement: Requirement, row: dict[str, str], allow_partial: bool) -> bool:
    return (
        result_matches(row, allow_partial)
        and value_matches(row, "media", requirement.media)
        and value_matches(row, "bus", requirement.bus)
        and value_matches(row, "sector_size", requirement.sector_size)
        and value_matches(row, "data_fs", requirement.data_fs)
        and image_matches(row, requirement.image_type)
    )


def covered_requirements(
    rows: list[dict[str, str]], allow_partial: bool
) -> tuple[list[Requirement], list[Requirement]]:
    covered: list[Requirement] = []
    missing: list[Requirement] = []
    for requirement in REQUIREMENTS:
        if any(row_matches(requirement, row, allow_partial) for row in rows):
            covered.append(requirement)
        else:
            missing.append(requirement)
    return covered, missing


def print_requirements(label: str, requirements: list[Requirement]) -> None:
    print(f"{label}: {len(requirements)}")
    for requirement in requirements:
        print(f"  - {requirement.name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, default=DEFAULT_CSV, help=f"matrix CSV path (default: {DEFAULT_CSV})")
    parser.add_argument(
        "--allow-partial",
        action="store_true",
        help="count partial rows as covered; default requires pass rows",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.csv.exists():
        print(f"hardware matrix not found: {args.csv}", file=sys.stderr)
        return 2

    rows = load_rows(args.csv)
    covered, missing = covered_requirements(rows, args.allow_partial)
    print(f"Checked {args.csv} ({len(rows)} data row(s))")
    print_requirements("Covered required rows", covered)
    print_requirements("Missing required rows", missing)
    return 1 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
