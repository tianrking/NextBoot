#!/usr/bin/env python3
"""Wrap a raw disk image as a VHD for NextBoot smoke tests."""

from __future__ import annotations

import argparse
import datetime as dt
import struct
import uuid
from pathlib import Path


FOOTER_SIZE = 512
DYNAMIC_HEADER_SIZE = 1024
VHD_EPOCH = dt.datetime(2000, 1, 1, tzinfo=dt.timezone.utc)


def be16(value: int) -> bytes:
    return value.to_bytes(2, "big")


def be32(value: int) -> bytes:
    return value.to_bytes(4, "big")


def be64(value: int) -> bytes:
    return value.to_bytes(8, "big")


def align_up(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def ceil_div(value: int, divisor: int) -> int:
    return (value + divisor - 1) // divisor


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


def vhd_checksum(data: bytearray, offset: int) -> int:
    original = data[offset : offset + 4]
    data[offset : offset + 4] = b"\x00\x00\x00\x00"
    checksum = (~sum(data) & 0xFFFFFFFF)
    data[offset : offset + 4] = original
    return checksum


def vhd_footer(size_bytes: int, source: Path, disk_type: int, data_offset: int) -> bytes:
    if size_bytes == 0 or size_bytes % 512:
        raise ValueError("VHD payload size must be a non-zero multiple of 512 bytes")

    footer = bytearray(FOOTER_SIZE)
    footer[0:8] = b"conectix"
    footer[8:12] = be32(0x00000002)
    footer[12:16] = be32(0x00010000)
    footer[16:24] = be64(data_offset)
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
    footer[60:64] = be32(disk_type)
    footer[68:84] = uuid.uuid5(
        uuid.NAMESPACE_URL,
        f"nextboot-vhd:{disk_type}:{source}",
    ).bytes
    footer[84] = 0

    footer[64:68] = be32(vhd_checksum(footer, 64))
    return bytes(footer)


def fixed_vhd(raw: bytes, source: Path) -> bytes:
    return raw + vhd_footer(len(raw), source, 2, 0xFFFFFFFFFFFFFFFF)


def dynamic_vhd_header(table_offset: int, max_table_entries: int, block_size: int) -> bytes:
    header = bytearray(DYNAMIC_HEADER_SIZE)
    header[0:8] = b"cxsparse"
    header[8:16] = be64(0xFFFFFFFFFFFFFFFF)
    header[16:24] = be64(table_offset)
    header[24:28] = be32(0x00010000)
    header[28:32] = be32(max_table_entries)
    header[32:36] = be32(block_size)
    header[36:40] = be32(vhd_checksum(header, 36))
    return bytes(header)


def dynamic_vhd(raw: bytes, source: Path) -> bytes:
    block_size = 2 * 1024 * 1024
    data_offset = FOOTER_SIZE
    header_offset = data_offset
    table_offset = header_offset + DYNAMIC_HEADER_SIZE
    block_count = ceil_div(len(raw), block_size)
    bat_size = align_up(block_count * 4, 512)
    data_start = table_offset + bat_size
    sectors_per_block = block_size // 512
    bitmap_size = align_up(ceil_div(sectors_per_block, 8), 512)

    image = bytearray()
    footer = vhd_footer(len(raw), source, 3, data_offset)
    image.extend(footer)
    image.extend(dynamic_vhd_header(table_offset, block_count, block_size))

    bat = bytearray(bat_size)
    next_block_offset = data_start
    for index in range(block_count):
        struct.pack_into(">I", bat, index * 4, next_block_offset // 512)
        next_block_offset += bitmap_size + block_size
    image.extend(bat)

    full_bitmap = bytes([0xFF]) * bitmap_size
    for index in range(block_count):
        start = index * block_size
        chunk = raw[start : start + block_size]
        image.extend(full_bitmap)
        image.extend(chunk)
        image.extend(bytes(block_size - len(chunk)))

    image.extend(footer)
    return bytes(image)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--format",
        choices=("fixed", "dynamic"),
        default="fixed",
        help="VHD variant to generate",
    )
    parser.add_argument("raw_image", type=Path)
    parser.add_argument("vhd_image", type=Path)
    args = parser.parse_args()

    raw = args.raw_image.read_bytes()
    args.vhd_image.parent.mkdir(parents=True, exist_ok=True)
    if args.format == "dynamic":
        data = dynamic_vhd(raw, args.raw_image.resolve())
    else:
        data = fixed_vhd(raw, args.raw_image.resolve())
    args.vhd_image.write_bytes(data)
    print(f"created {args.vhd_image} ({args.vhd_image.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
