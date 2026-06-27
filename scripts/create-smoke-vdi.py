#!/usr/bin/env python3
"""Wrap a raw disk image as a dynamic VDI for NextBoot smoke tests."""

from __future__ import annotations

import argparse
import struct
import uuid
from pathlib import Path


HEADER_SIZE = 512
HEADER_STRUCT_SIZE = 0x180
SIGNATURE = 0xBEDA107F
VERSION_1_1 = 0x00010001
IMAGE_TYPE_DYNAMIC = 1
BLOCK_SIZE = 1024 * 1024
SECTOR_SIZE = 512


def align_up(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def ceil_div(value: int, divisor: int) -> int:
    return (value + divisor - 1) // divisor


def put_u32(header: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", header, offset, value)


def put_u64(header: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<Q", header, offset, value)


def put_uuid(header: bytearray, offset: int, name: str) -> None:
    header[offset : offset + 16] = uuid.uuid5(uuid.NAMESPACE_URL, name).bytes_le


def dynamic_vdi(raw: bytes, source: Path) -> bytes:
    if not raw or len(raw) % SECTOR_SIZE:
        raise ValueError("VDI payload size must be a non-zero multiple of 512 bytes")

    block_count = ceil_div(len(raw), BLOCK_SIZE)
    blocks_offset = HEADER_SIZE
    map_size = align_up(block_count * 4, SECTOR_SIZE)
    data_offset = blocks_offset + map_size

    header = bytearray(HEADER_SIZE)
    file_info = b"<<< NextBoot Smoke Dynamic VDI >>>\n"
    header[: len(file_info)] = file_info
    put_u32(header, 0x40, SIGNATURE)
    put_u32(header, 0x44, VERSION_1_1)
    put_u32(header, 0x48, HEADER_STRUCT_SIZE)
    put_u32(header, 0x4C, IMAGE_TYPE_DYNAMIC)
    put_u32(header, 0x154, blocks_offset)
    put_u32(header, 0x158, data_offset)
    put_u32(header, 0x168, SECTOR_SIZE)
    put_u64(header, 0x170, len(raw))
    put_u32(header, 0x178, BLOCK_SIZE)
    put_u32(header, 0x17C, 0)
    put_u32(header, 0x180, block_count)
    put_u32(header, 0x184, block_count)
    base_uuid = f"nextboot-vdi:{source.resolve()}"
    put_uuid(header, 0x188, f"{base_uuid}:create")
    put_uuid(header, 0x198, f"{base_uuid}:modify")
    put_uuid(header, 0x1A8, f"{base_uuid}:linkage")
    put_uuid(header, 0x1B8, f"{base_uuid}:parent")
    assert len(header) == HEADER_SIZE

    block_map = bytearray(map_size)
    for index in range(block_count):
        struct.pack_into("<I", block_map, index * 4, index)

    image = bytearray()
    image.extend(header)
    image.extend(block_map)
    for index in range(block_count):
        start = index * BLOCK_SIZE
        chunk = raw[start : start + BLOCK_SIZE]
        image.extend(chunk)
        image.extend(bytes(BLOCK_SIZE - len(chunk)))
    return bytes(image)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_image", type=Path)
    parser.add_argument("vdi_image", type=Path)
    args = parser.parse_args()

    raw = args.raw_image.read_bytes()
    args.vdi_image.parent.mkdir(parents=True, exist_ok=True)
    args.vdi_image.write_bytes(dynamic_vdi(raw, args.raw_image))
    print(f"created {args.vdi_image} ({args.vdi_image.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
