"""Shared real-hardware compatibility matrix helpers."""

from __future__ import annotations

import csv
from dataclasses import dataclass
from pathlib import Path


DEFAULT_CSV = Path("docs/hardware/hardware-matrix.csv")
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


@dataclass(frozen=True)
class Requirement:
    name: str
    media: tuple[str, ...]
    bus: tuple[str, ...]
    sector_size: tuple[str, ...]
    data_fs: tuple[str, ...]
    image_type: tuple[str, ...]


REQUIREMENTS: tuple[Requirement, ...] = (
    Requirement(
        "Internal SSD NVMe 512 exFAT ISO",
        ("fixed", "nvme"),
        ("nvme",),
        ("512",),
        ("exfat",),
        ("iso",),
    ),
    Requirement(
        "Internal SSD NVMe 4096 exFAT ISO",
        ("fixed", "nvme"),
        ("nvme",),
        ("4096",),
        ("exfat",),
        ("iso",),
    ),
    Requirement("USB stick FAT32 ISO", ("usb",), ("usb",), ("512",), ("fat32",), ("iso",)),
    Requirement(
        "USB SSD enclosure NTFS Windows WIMBOOT",
        ("usb", "enclosure"),
        ("usb",),
        ("512",),
        ("ntfs",),
        ("windows", "wimboot"),
    ),
    Requirement(
        "USB SSD enclosure 4096 exFAT VHDX",
        ("usb", "enclosure"),
        ("usb",),
        ("4096",),
        ("exfat",),
        ("vhdx",),
    ),
    Requirement("SATA SSD NTFS ISO", ("fixed", "sata"), ("sata", "ahci"), ("512",), ("ntfs",), ("iso",)),
    Requirement("SD reader FAT32 ISO", ("sd",), ("sd",), ("512",), ("fat32",), ("iso",)),
    Requirement(
        "Linux-prepared ext4 plugins",
        ("fixed", "nvme", "usb", "enclosure"),
        ("nvme", "usb"),
        ("4096",),
        ("ext4",),
        ("linux", "plugins"),
    ),
    Requirement(
        "Linux-prepared XFS VLNK ISO",
        ("fixed", "nvme", "usb", "enclosure"),
        ("nvme", "usb"),
        ("4096",),
        ("xfs",),
        ("vlnk", "iso"),
    ),
    Requirement(
        "UDF Windows ISO",
        ("fixed", "nvme", "sata", "usb", "sd", "enclosure", "other"),
        ("nvme", "sata", "ahci", "usb", "sd", "virtio", "other"),
        ("512", "4096"),
        ("udf",),
        ("windows", "iso"),
    ),
)


def normalize(value: str | None) -> str:
    return (value or "").strip().lower().replace("_", "-")


def load_rows(path: Path) -> tuple[list[dict[str, str]], list[str]]:
    with path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        rows = list(reader)
    errors: list[str] = []
    if reader.fieldnames != EXPECTED_COLUMNS:
        errors.append(f"unexpected CSV header: {reader.fieldnames!r}")
    return rows, errors


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


def matching_rows(
    requirement: Requirement,
    rows: list[dict[str, str]],
    allow_partial: bool,
) -> list[dict[str, str]]:
    return [row for row in rows if row_matches(requirement, row, allow_partial)]


def covered_requirements(
    rows: list[dict[str, str]], allow_partial: bool
) -> tuple[list[Requirement], list[Requirement]]:
    covered: list[Requirement] = []
    missing: list[Requirement] = []
    for requirement in REQUIREMENTS:
        if matching_rows(requirement, rows, allow_partial):
            covered.append(requirement)
        else:
            missing.append(requirement)
    return covered, missing


def result_counts(rows: list[dict[str, str]]) -> dict[str, int]:
    counts = {name: 0 for name in ("pass", "partial", "fail", "blocked", "unknown")}
    for row in rows:
        result = normalize(row.get("result"))
        counts[result if result in counts else "unknown"] += 1
    return counts
