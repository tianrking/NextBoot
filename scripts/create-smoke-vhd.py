#!/usr/bin/env python3
"""Wrap a raw disk image as a fixed-size VHD for NextBoot smoke tests."""

from __future__ import annotations

import argparse
import datetime as dt
import struct
import uuid
from pathlib import Path


FOOTER_SIZE = 512
VHD_EPOCH = dt.datetime(2000, 1, 1, tzinfo=dt.timezone.utc)


def be16(value: int) -> bytes:
    return value.to_bytes(2, "big")


def be32(value: int) -> bytes:
    return value.to_bytes(4, "big")


def be64(value: int) -> bytes:
    return value.to_bytes(8, "big")


def chs_geometry(size_bytes: int) -> tuple[int, int, int]:
    total_sectors = min(size_bytes // 512, 65_535 * 16 * 255)
    if total_sectors >= 65_535 * 16 * 63:
        sectors_per_track = 255
        heads = 16
        cylinders = total_sectors // sectors_per_track // heads
    else:
        sectors_per_track = 17
        cylinders_times_heads = total_sectors // sectors_per_track
        heads = (cylinders_times_heads + 1023) // 1024
        if heads < 4:
            heads = 4
        if cylinders_times_heads >= heads * 1024 or heads > 16:
            sectors_per_track = 31
            heads = 16
            cylinders_times_heads = total_sectors // sectors_per_track
        if cylinders_times_heads >= heads * 1024:
            sectors_per_track = 63
            heads = 16
            cylinders_times_heads = total_sectors // sectors_per_track
        cylinders = cylinders_times_heads // heads
    return int(cylinders), int(heads), int(sectors_per_track)


def fixed_vhd_footer(size_bytes: int, source: Path) -> bytes:
    if size_bytes == 0 or size_bytes % 512:
        raise ValueError("fixed VHD payload size must be a non-zero multiple of 512 bytes")

    footer = bytearray(FOOTER_SIZE)
    footer[0:8] = b"conectix"
    footer[8:12] = be32(0x00000002)
    footer[12:16] = be32(0x00010000)
    footer[16:24] = be64(0xFFFFFFFFFFFFFFFF)
    timestamp = int((dt.datetime.now(dt.timezone.utc) - VHD_EPOCH).total_seconds())
    footer[24:28] = be32(timestamp)
    footer[28:32] = b"nxtb"
    footer[32:36] = be32(0x00010000)
    footer[36:40] = b"Wi2k"
    footer[40:48] = be64(size_bytes)
    footer[48:56] = be64(size_bytes)
    cylinders, heads, sectors = chs_geometry(size_bytes)
    footer[56:58] = be16(cylinders)
    footer[58] = heads
    footer[59] = sectors
    footer[60:64] = be32(2)
    footer[68:84] = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-fixed-vhd:{source}").bytes
    footer[84] = 0

    checksum = (~sum(footer) & 0xFFFFFFFF)
    footer[64:68] = be32(checksum)
    return bytes(footer)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_image", type=Path)
    parser.add_argument("vhd_image", type=Path)
    args = parser.parse_args()

    raw = args.raw_image.read_bytes()
    args.vhd_image.parent.mkdir(parents=True, exist_ok=True)
    args.vhd_image.write_bytes(raw + fixed_vhd_footer(len(raw), args.raw_image.resolve()))
    print(f"created {args.vhd_image} ({args.vhd_image.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
