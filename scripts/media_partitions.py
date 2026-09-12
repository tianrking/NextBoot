#!/usr/bin/env python3
"""Identify an existing NextBoot disk by partition metadata, never partition order."""

from __future__ import annotations

import argparse
import json
import os
import plistlib
import re
import subprocess
import sys
from dataclasses import dataclass

ESP_GUID = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
BASIC_GUID = "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"
ESP_LABELS = {"NEXBOOT", "NEXTBOOT", "VTOYEFI", "NEXBOOT_EFI"}


@dataclass(frozen=True)
class Partition:
    path: str
    kind: str
    label: str
    filesystem: str


def select_partitions(parts: list[Partition], force: bool = False) -> tuple[str, str]:
    """Force may waive a missing data label, never an unverified ESP."""
    esp = [p for p in parts if p.kind.lower() == ESP_GUID]
    data = [p for p in parts if p.label == "NEXTDATA"]
    if len(esp) != 1:
        raise ValueError(f"expected one EFI System Partition, found {len(esp)}")
    if esp[0].filesystem.lower() not in {"vfat", "fat", "fat16", "fat32", "msdos"}:
        raise ValueError("EFI System Partition must have a FAT filesystem")
    if len(data) > 1:
        raise ValueError("multiple NEXTDATA partitions; refusing ambiguous disk")
    if data and (data[0].path == esp[0].path or data[0].kind.lower() != BASIC_GUID):
        raise ValueError("NEXTDATA must be a separate basic-data partition")
    if not data and not force:
        raise ValueError("NEXTDATA was not detected; --force only waives the data label check")
    if not data and esp[0].label.upper() not in ESP_LABELS:
        raise ValueError("without NEXTDATA, the ESP must have a known NextBoot label")
    return esp[0].path, data[0].path if data else "-"


def command(args: list[str]) -> bytes:
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def linux_partitions(device: str, allow_loop: bool = False) -> list[Partition]:
    payload = json.loads(command([
        "lsblk", "--json", "--paths", "--output", "NAME,TYPE,PARTTYPE,LABEL,FSTYPE", device,
    ]))
    disks = payload.get("blockdevices", [])
    is_loop = (len(disks) == 1 and disks[0].get("type") == "loop"
               and allow_loop and re.fullmatch(r"/dev/loop[0-9]+", device))
    if len(disks) != 1 or (disks[0].get("type") != "disk" and not is_loop):
        raise ValueError("select a whole disk, not a partition or mapped device")
    disk = disks[0]
    if disk.get("name") != device:
        raise ValueError("device inventory does not match the selected disk")
    pattern = re.escape(device) + (r"p[0-9]+" if device[-1].isdigit() else r"[0-9]+")
    parts = []
    for item in disk.get("children", []):
        path = item.get("name", "")
        if item.get("type") != "part" or not re.fullmatch(pattern, path):
            raise ValueError("unexpected child device; refusing ambiguous disk topology")
        if item.get("children"):
            raise ValueError("partition has active mapped devices; refusing update")
        parts.append(Partition(path, item.get("parttype") or "", item.get("label") or "", item.get("fstype") or ""))
    return parts


def macos_partitions(device: str) -> list[Partition]:
    identifier = device.removeprefix("/dev/")
    info = plistlib.loads(command(["diskutil", "info", "-plist", device]))
    if not info.get("Whole") or info.get("DeviceIdentifier") != identifier:
        raise ValueError("select the requested whole physical disk")
    listing = plistlib.loads(command(["diskutil", "list", "-plist", device]))
    disks = listing.get("AllDisksAndPartitions", [])
    if len(disks) != 1 or disks[0].get("DeviceIdentifier") != identifier:
        raise ValueError("device inventory does not match the selected disk")
    parts = []
    for entry in disks[0].get("Partitions", []):
        part_id = entry.get("DeviceIdentifier", "")
        if not re.fullmatch(re.escape(identifier) + r"s[0-9]+", part_id):
            raise ValueError("partition does not belong to the selected disk")
        part = plistlib.loads(command(["diskutil", "info", "-plist", f"/dev/{part_id}"]))
        if part.get("ParentWholeDisk") != identifier or part.get("DeviceIdentifier") != part_id:
            raise ValueError("partition identity changed during inspection")
        content = part.get("Content", entry.get("Content", ""))
        kind = {"EFI": ESP_GUID, "Microsoft Basic Data": BASIC_GUID}.get(content, content)
        parts.append(Partition(f"/dev/{part_id}", kind, part.get("VolumeName", ""), part.get("FilesystemType", "")))
    return parts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", choices=("linux", "darwin"), required=True)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--allow-loop", action="store_true", help="allow whole Linux loop disks for explicit image testing")
    parser.add_argument("device")
    args = parser.parse_args()
    try:
        device = args.device
        if args.host == "darwin":
            if not re.fullmatch(r"/dev/disk[0-9]+", device):
                raise ValueError("expected /dev/diskN")
            parts = macos_partitions(device)
        else:
            device = os.path.realpath(device)
            if not re.fullmatch(r"/dev/[A-Za-z0-9._-]+", device):
                raise ValueError("expected a whole disk under /dev")
            parts = linux_partitions(device, args.allow_loop)
        esp, data = select_partitions(parts, args.force)
        print(f"{esp}\t{data}")
        return 0
    except (ValueError, OSError, subprocess.CalledProcessError, plistlib.InvalidFileException) as error:
        print(f"Cannot identify NextBoot partitions: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
