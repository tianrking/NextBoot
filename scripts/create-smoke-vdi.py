#!/usr/bin/env python3
"""Wrap a raw disk image as a VDI for NextBoot smoke tests."""

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
IMAGE_TYPE_STATIC = 2
IMAGE_TYPE_DIFFERENCING = 4
DISCARDED_BLOCK = 0xFFFFFFFE
UNALLOCATED_BLOCK = 0xFFFFFFFF
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


def is_zero_block(chunk: bytes) -> bool:
    return not any(chunk)


def vdi_image(
    raw: bytes,
    source: Path,
    image_format: str,
    sparse_mode: str,
    link_to_parent: bool,
) -> bytes:
    if not raw or len(raw) % SECTOR_SIZE:
        raise ValueError("VDI payload size must be a non-zero multiple of 512 bytes")
    if image_format == "static" and sparse_mode != "allocated":
        raise ValueError("static VDI cannot use sparse block-map entries")

    block_count = ceil_div(len(raw), BLOCK_SIZE)
    chunks = [raw[index * BLOCK_SIZE : (index + 1) * BLOCK_SIZE] for index in range(block_count)]
    allocated = [sparse_mode == "allocated" or not is_zero_block(chunk) for chunk in chunks]
    if image_format == "differencing" and all(allocated):
        raise ValueError("differencing VDI smoke image needs at least one sparse parent-backed block")
    allocated_count = sum(1 for value in allocated if value)
    blocks_offset = HEADER_SIZE
    map_size = align_up(block_count * 4, SECTOR_SIZE)
    data_offset = blocks_offset + map_size
    image_type = {
        "dynamic": IMAGE_TYPE_DYNAMIC,
        "static": IMAGE_TYPE_STATIC,
        "differencing": IMAGE_TYPE_DIFFERENCING,
    }[image_format]

    header = bytearray(HEADER_SIZE)
    file_info = f"<<< NextBoot Smoke {image_format.title()} VDI >>>\n".encode()
    header[: len(file_info)] = file_info
    put_u32(header, 0x40, SIGNATURE)
    put_u32(header, 0x44, VERSION_1_1)
    put_u32(header, 0x48, HEADER_STRUCT_SIZE)
    put_u32(header, 0x4C, image_type)
    put_u32(header, 0x154, blocks_offset)
    put_u32(header, 0x158, data_offset)
    put_u32(header, 0x168, SECTOR_SIZE)
    put_u64(header, 0x170, len(raw))
    put_u32(header, 0x178, BLOCK_SIZE)
    put_u32(header, 0x17C, 0)
    put_u32(header, 0x180, block_count)
    put_u32(header, 0x184, allocated_count)
    base_uuid = f"nextboot-vdi:{source.resolve()}"
    put_uuid(header, 0x188, f"{base_uuid}:create")
    put_uuid(header, 0x198, f"{base_uuid}:modify")
    if link_to_parent:
        put_uuid(header, 0x1A8, f"{base_uuid}:create")
        put_uuid(header, 0x1B8, f"{base_uuid}:modify")
    else:
        put_uuid(header, 0x1A8, f"{base_uuid}:linkage")
        put_uuid(header, 0x1B8, f"{base_uuid}:parent")
    assert len(header) == HEADER_SIZE

    block_map = bytearray(map_size)
    physical_index = 0
    for index, is_allocated in enumerate(allocated):
        if is_allocated:
            map_entry = physical_index
            physical_index += 1
        elif sparse_mode == "discarded":
            map_entry = DISCARDED_BLOCK
        else:
            map_entry = UNALLOCATED_BLOCK
        struct.pack_into("<I", block_map, index * 4, map_entry)

    image = bytearray()
    image.extend(header)
    image.extend(block_map)
    for chunk, is_allocated in zip(chunks, allocated):
        if not is_allocated:
            continue
        image.extend(chunk)
        image.extend(bytes(BLOCK_SIZE - len(chunk)))
    return bytes(image)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("dynamic", "static", "differencing"), default="dynamic")
    parser.add_argument(
        "--sparse-mode",
        choices=("allocated", "unallocated", "discarded"),
        default="allocated",
        help="how dynamic VDI encodes all-zero payload blocks",
    )
    parser.add_argument(
        "--sparse",
        action="store_const",
        const="unallocated",
        dest="sparse_mode",
        help="alias for --sparse-mode unallocated",
    )
    parser.add_argument(
        "--link-to-parent",
        action="store_true",
        help="for differencing images, point linkage UUIDs at a parent generated from the same raw image",
    )
    parser.add_argument("raw_image", type=Path)
    parser.add_argument("vdi_image", type=Path)
    args = parser.parse_args()

    raw = args.raw_image.read_bytes()
    args.vdi_image.parent.mkdir(parents=True, exist_ok=True)
    args.vdi_image.write_bytes(
        vdi_image(raw, args.raw_image, args.format, args.sparse_mode, args.link_to_parent)
    )
    print(f"created {args.vdi_image} ({args.vdi_image.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
