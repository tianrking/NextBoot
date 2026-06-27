#!/usr/bin/env python3
"""Create a minimal ISO9660 image with /EFI/BOOT/BOOTX64.EFI."""

from __future__ import annotations

import argparse
import math
import os
from pathlib import Path


SECTOR_SIZE = 2048
SYSTEM_ID = b"NEXTBOOT"


def pad_ascii(text: str, size: int) -> bytes:
    data = text.upper().encode("ascii", "ignore")[:size]
    return data.ljust(size, b" ")


def both_endian_u16(value: int) -> bytes:
    return value.to_bytes(2, "little") + value.to_bytes(2, "big")


def both_endian_u32(value: int) -> bytes:
    return value.to_bytes(4, "little") + value.to_bytes(4, "big")


def directory_timestamp() -> bytes:
    return bytes([126, 1, 1, 0, 0, 0, 0])


def directory_record(name: bytes, extent_lba: int, data_length: int, flags: int) -> bytes:
    length = 33 + len(name)
    if length % 2 != 0:
        length += 1

    record = bytearray(length)
    record[0] = length
    record[1] = 0
    record[2:10] = both_endian_u32(extent_lba)
    record[10:18] = both_endian_u32(data_length)
    record[18:25] = directory_timestamp()
    record[25] = flags
    record[26] = 0
    record[27] = 0
    record[28:32] = both_endian_u16(1)
    record[32] = len(name)
    record[33 : 33 + len(name)] = name
    return bytes(record)


def path_table_record(name: bytes, extent_lba: int, parent_index: int, endian: str) -> bytes:
    record = bytearray()
    record.append(len(name))
    record.append(0)
    record.extend(extent_lba.to_bytes(4, endian))
    record.extend(parent_index.to_bytes(2, endian))
    record.extend(name)
    if len(name) % 2 != 0:
        record.append(0)
    return bytes(record)


def sector(data: bytes = b"") -> bytes:
    if len(data) > SECTOR_SIZE:
        raise ValueError("sector payload is too large")
    return data + bytes(SECTOR_SIZE - len(data))


def directory_sector(records: list[bytes]) -> bytes:
    return sector(b"".join(records))


def primary_volume_descriptor(
    label: str,
    total_sectors: int,
    root_lba: int,
    root_size: int,
    path_table_size: int,
    le_path_table_lba: int,
    be_path_table_lba: int,
) -> bytes:
    pvd = bytearray(SECTOR_SIZE)
    pvd[0] = 1
    pvd[1:6] = b"CD001"
    pvd[6] = 1
    pvd[8:40] = pad_ascii(SYSTEM_ID.decode("ascii"), 32)
    pvd[40:72] = pad_ascii(label, 32)
    pvd[80:88] = both_endian_u32(total_sectors)
    pvd[120:124] = both_endian_u16(1)
    pvd[124:128] = both_endian_u16(1)
    pvd[128:132] = both_endian_u16(SECTOR_SIZE)
    pvd[132:140] = both_endian_u32(path_table_size)
    pvd[140:144] = le_path_table_lba.to_bytes(4, "little")
    pvd[144:148] = (0).to_bytes(4, "little")
    pvd[148:152] = be_path_table_lba.to_bytes(4, "big")
    pvd[152:156] = (0).to_bytes(4, "big")
    pvd[156:190] = directory_record(b"\x00", root_lba, root_size, 0x02)
    pvd[813:830] = b"2026010100000000\x00"
    pvd[830:847] = b"2026010100000000\x00"
    pvd[847:864] = b"0000000000000000\x00"
    pvd[864:881] = b"0000000000000000\x00"
    pvd[881] = 1
    return bytes(pvd)


def volume_descriptor_terminator() -> bytes:
    data = bytearray(SECTOR_SIZE)
    data[0] = 255
    data[1:6] = b"CD001"
    data[6] = 1
    return bytes(data)


def write_smoke_iso(output: Path, efi: Path, label: str) -> None:
    efi_data = efi.read_bytes()
    file_sectors = max(1, math.ceil(len(efi_data) / SECTOR_SIZE))

    pvd_lba = 16
    terminator_lba = 17
    le_path_table_lba = 18
    be_path_table_lba = 19
    root_lba = 20
    efi_dir_lba = 21
    boot_dir_lba = 22
    file_lba = 23
    total_sectors = file_lba + file_sectors

    path_entries = [
        (b"\x00", root_lba, 1),
        (b"EFI", efi_dir_lba, 1),
        (b"BOOT", boot_dir_lba, 2),
    ]
    le_path_table = b"".join(
        path_table_record(name, lba, parent, "little")
        for name, lba, parent in path_entries
    )
    be_path_table = b"".join(
        path_table_record(name, lba, parent, "big")
        for name, lba, parent in path_entries
    )
    path_table_size = len(le_path_table)

    root_dir = directory_sector(
        [
            directory_record(b"\x00", root_lba, SECTOR_SIZE, 0x02),
            directory_record(b"\x01", root_lba, SECTOR_SIZE, 0x02),
            directory_record(b"EFI", efi_dir_lba, SECTOR_SIZE, 0x02),
        ]
    )
    efi_dir = directory_sector(
        [
            directory_record(b"\x00", efi_dir_lba, SECTOR_SIZE, 0x02),
            directory_record(b"\x01", root_lba, SECTOR_SIZE, 0x02),
            directory_record(b"BOOT", boot_dir_lba, SECTOR_SIZE, 0x02),
        ]
    )
    boot_dir = directory_sector(
        [
            directory_record(b"\x00", boot_dir_lba, SECTOR_SIZE, 0x02),
            directory_record(b"\x01", efi_dir_lba, SECTOR_SIZE, 0x02),
            directory_record(b"BOOTX64.EFI;1", file_lba, len(efi_data), 0x00),
        ]
    )

    image = bytearray(total_sectors * SECTOR_SIZE)
    image[pvd_lba * SECTOR_SIZE : (pvd_lba + 1) * SECTOR_SIZE] = (
        primary_volume_descriptor(
            label,
            total_sectors,
            root_lba,
            SECTOR_SIZE,
            path_table_size,
            le_path_table_lba,
            be_path_table_lba,
        )
    )
    image[terminator_lba * SECTOR_SIZE : (terminator_lba + 1) * SECTOR_SIZE] = (
        volume_descriptor_terminator()
    )
    image[le_path_table_lba * SECTOR_SIZE : (le_path_table_lba + 1) * SECTOR_SIZE] = sector(
        le_path_table
    )
    image[be_path_table_lba * SECTOR_SIZE : (be_path_table_lba + 1) * SECTOR_SIZE] = sector(
        be_path_table
    )
    image[root_lba * SECTOR_SIZE : (root_lba + 1) * SECTOR_SIZE] = root_dir
    image[efi_dir_lba * SECTOR_SIZE : (efi_dir_lba + 1) * SECTOR_SIZE] = efi_dir
    image[boot_dir_lba * SECTOR_SIZE : (boot_dir_lba + 1) * SECTOR_SIZE] = boot_dir

    file_offset = file_lba * SECTOR_SIZE
    image[file_offset : file_offset + len(efi_data)] = efi_data

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(image)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--efi", required=True, type=Path, help="BOOTX64.EFI payload")
    parser.add_argument("--label", default="NEXTSMOKE", help="ISO9660 volume label")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.efi.is_file():
        raise SystemExit(f"EFI payload not found: {args.efi}")
    if len(args.label) > 32:
        raise SystemExit("--label must fit in 32 ASCII characters")
    write_smoke_iso(args.output, args.efi, args.label)
    print(f"created {args.output} ({os.path.getsize(args.output)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
