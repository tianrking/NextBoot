#!/usr/bin/env python3
"""Wrap a raw disk image as a minimal VHDX for NextBoot smoke tests."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


MIB = 1024 * 1024
HEADER_SECTION_SIZE = MIB
METADATA_OFFSET = MIB
BAT_OFFSET = 2 * MIB
PAYLOAD_OFFSET = 3 * MIB
REGION_TABLE_OFFSET = 192 * 1024
REGION_TABLE_SIZE = 64 * 1024
BLOCK_SIZE = 2 * MIB
LOGICAL_SECTOR_SIZE = 512
PHYSICAL_SECTOR_SIZE = 4096
BAT_STATE_FULLY_PRESENT = 6
BAT_STATE_PARTIALLY_PRESENT = 7
BAT_STATE_ZERO = 2
FILE_PARAMETERS_HAS_PARENT = 1 << 1

BAT_REGION_GUID = bytes.fromhex("6677c22d23f600429d64115e9bfd4a08")
METADATA_REGION_GUID = bytes.fromhex("06a27c8b90479a4bb8fe575f050f886e")
FILE_PARAMETERS_GUID = bytes.fromhex("3767a1ca36fa434db3b633f0aa44e76b")
VIRTUAL_DISK_SIZE_GUID = bytes.fromhex("2442a52f1bcd7648b2115dbed83bf4b8")
LOGICAL_SECTOR_SIZE_GUID = bytes.fromhex("1dbf41816fa90947ba47f233a8faab5f")
PHYSICAL_SECTOR_SIZE_GUID = bytes.fromhex("c748a3cd5d4471449cc9e9885251c556")


def align_up(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def ceil_div(value: int, divisor: int) -> int:
    return (value + divisor - 1) // divisor


def put_u16(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", data, offset, value)


def put_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def put_u64(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<Q", data, offset, value)


def write_region_entry(
    table: bytearray,
    offset: int,
    guid: bytes,
    file_offset: int,
    length: int,
    flags: int,
) -> None:
    table[offset : offset + 16] = guid
    put_u64(table, offset + 16, file_offset)
    put_u32(table, offset + 24, length)
    put_u32(table, offset + 28, flags)


def write_metadata_entry(
    metadata: bytearray,
    offset: int,
    guid: bytes,
    item_offset: int,
    length: int,
) -> None:
    metadata[offset : offset + 16] = guid
    put_u32(metadata, offset + 16, item_offset)
    put_u32(metadata, offset + 20, length)
    put_u32(metadata, offset + 24, 0x6)


def is_zero_block(chunk: bytes) -> bool:
    return not any(chunk)


def header_section() -> bytes:
    header = bytearray(HEADER_SECTION_SIZE)
    header[0:8] = b"vhdxfile"
    header[REGION_TABLE_OFFSET : REGION_TABLE_OFFSET + 4] = b"regi"
    put_u32(header, REGION_TABLE_OFFSET + 8, 2)
    write_region_entry(header, REGION_TABLE_OFFSET + 16, BAT_REGION_GUID, BAT_OFFSET, MIB, 1)
    write_region_entry(
        header,
        REGION_TABLE_OFFSET + 48,
        METADATA_REGION_GUID,
        METADATA_OFFSET,
        MIB,
        1,
    )
    return bytes(header)


def metadata_region(virtual_size: int, has_parent: bool) -> bytes:
    metadata = bytearray(MIB)
    metadata[0:8] = b"metadata"
    put_u16(metadata, 10, 4)

    write_metadata_entry(metadata, 32, FILE_PARAMETERS_GUID, 0x10000, 8)
    put_u32(metadata, 0x10000, BLOCK_SIZE)
    put_u32(metadata, 0x10004, FILE_PARAMETERS_HAS_PARENT if has_parent else 0)

    write_metadata_entry(metadata, 64, VIRTUAL_DISK_SIZE_GUID, 0x10008, 8)
    put_u64(metadata, 0x10008, virtual_size)

    write_metadata_entry(metadata, 96, LOGICAL_SECTOR_SIZE_GUID, 0x10010, 4)
    put_u32(metadata, 0x10010, LOGICAL_SECTOR_SIZE)

    write_metadata_entry(metadata, 128, PHYSICAL_SECTOR_SIZE_GUID, 0x10014, 4)
    put_u32(metadata, 0x10014, PHYSICAL_SECTOR_SIZE)
    return bytes(metadata)


def payload_bat_index(payload_index: int, chunk_ratio: int) -> int:
    return payload_index + payload_index // chunk_ratio


def sector_bitmap_bat_index(payload_index: int, chunk_ratio: int) -> int:
    return (payload_index // chunk_ratio) * (chunk_ratio + 1) + chunk_ratio


def bat_region(
    block_offsets: list[int | None],
    bitmap_offset: int | None,
    partial_present: bool,
) -> bytes:
    bat = bytearray(MIB)
    chunk_ratio = (1 << 23) * LOGICAL_SECTOR_SIZE // BLOCK_SIZE
    for index, file_offset in enumerate(block_offsets):
        bat_index = payload_bat_index(index, chunk_ratio)
        if file_offset is None:
            raw_entry = BAT_STATE_ZERO
        elif partial_present:
            raw_entry = ((file_offset // MIB) << 20) | BAT_STATE_PARTIALLY_PRESENT
        else:
            raw_entry = ((file_offset // MIB) << 20) | BAT_STATE_FULLY_PRESENT
        put_u64(bat, bat_index * 8, raw_entry)
    if bitmap_offset is not None:
        bitmap_index = sector_bitmap_bat_index(0, chunk_ratio)
        raw_entry = ((bitmap_offset // MIB) << 20) | BAT_STATE_FULLY_PRESENT
        put_u64(bat, bitmap_index * 8, raw_entry)
    return bytes(bat)


def vhdx(raw: bytes, sparse: bool, partial_present: bool) -> bytes:
    if not raw or len(raw) % LOGICAL_SECTOR_SIZE:
        raise ValueError("VHDX payload size must be a non-zero multiple of 512 bytes")
    if sparse and partial_present:
        raise ValueError("--sparse and --partial-present cannot be combined")

    block_count = ceil_div(len(raw), BLOCK_SIZE)
    chunks = [raw[index * BLOCK_SIZE : (index + 1) * BLOCK_SIZE] for index in range(block_count)]
    allocated = [not sparse or not is_zero_block(chunk) for chunk in chunks]
    block_offsets: list[int | None] = []
    next_payload_offset = PAYLOAD_OFFSET
    bitmap_offset = None
    if partial_present:
        bitmap_offset = next_payload_offset
        next_payload_offset += MIB
    for is_allocated in allocated:
        if is_allocated:
            block_offsets.append(next_payload_offset)
            next_payload_offset += BLOCK_SIZE
        else:
            block_offsets.append(None)

    image = bytearray()
    image.extend(header_section())
    image.extend(metadata_region(len(raw), partial_present))
    image.extend(bat_region(block_offsets, bitmap_offset, partial_present))
    assert len(image) == PAYLOAD_OFFSET

    if bitmap_offset is not None:
        image.extend(bytes([0xFF]) * MIB)

    for chunk, is_allocated in zip(chunks, allocated):
        if not is_allocated:
            continue
        image.extend(chunk)
        image.extend(bytes(BLOCK_SIZE - len(chunk)))
    return bytes(image)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sparse", action="store_true", help="encode zero blocks as sparse")
    parser.add_argument(
        "--partial-present",
        action="store_true",
        help="encode allocated blocks as partially-present with a full sector bitmap",
    )
    parser.add_argument("raw_image", type=Path)
    parser.add_argument("vhdx_image", type=Path)
    args = parser.parse_args()

    raw = args.raw_image.read_bytes()
    args.vhdx_image.parent.mkdir(parents=True, exist_ok=True)
    args.vhdx_image.write_bytes(vhdx(raw, args.sparse, args.partial_present))
    print(f"created {args.vhdx_image} ({args.vhdx_image.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
