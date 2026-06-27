#!/usr/bin/env python3
"""Create a minimal EFI smoke ISO9660 image with El Torito metadata."""

from __future__ import annotations

import argparse
import math
import os
from pathlib import Path


SECTOR_SIZE = 2048
SYSTEM_ID = b"NEXTBOOT"
EL_TORITO_ID = b"EL TORITO SPECIFICATION"
EL_TORITO_PLATFORM_EFI = 0xEF
PROFILE_GENERIC = "generic"
PROFILE_WINDOWS = "windows"
PROFILE_LINUX = "linux"
LINUX_SMOKE_INITRD = b"070701NEXTBOOT SMOKE INITRD\n"


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


def eltorito_boot_record(catalog_lba: int) -> bytes:
    data = bytearray(SECTOR_SIZE)
    data[0] = 0
    data[1:6] = b"CD001"
    data[6] = 1
    data[7 : 7 + len(EL_TORITO_ID)] = EL_TORITO_ID
    data[0x47:0x4B] = catalog_lba.to_bytes(4, "little")
    return bytes(data)


def eltorito_boot_catalog(boot_image_lba: int, boot_image_size: int) -> bytes:
    catalog = bytearray(SECTOR_SIZE)
    catalog[0] = 0x01
    catalog[1] = EL_TORITO_PLATFORM_EFI
    catalog[30] = 0x55
    catalog[31] = 0xAA
    checksum = (-sum(
        int.from_bytes(catalog[index : index + 2], "little")
        for index in range(0, 32, 2)
    )) & 0xFFFF
    catalog[28:30] = checksum.to_bytes(2, "little")

    sector_count = max(1, math.ceil(boot_image_size / 512))
    if sector_count > 0xFFFF:
        raise ValueError("El Torito boot image is too large")
    catalog[32] = 0x88
    catalog[33] = 0x00
    catalog[34:36] = (0).to_bytes(2, "little")
    catalog[36] = 0x00
    catalog[38:40] = sector_count.to_bytes(2, "little")
    catalog[40:44] = boot_image_lba.to_bytes(4, "little")
    return bytes(catalog)


def align_up(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def read_u16(data: bytes | bytearray, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 2], "little")


def read_u32(data: bytes | bytearray, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 4], "little")


def write_u32(data: bytearray, offset: int, value: int) -> None:
    data[offset : offset + 4] = value.to_bytes(4, "little")


def linux_smoke_kernel(efi_data: bytes) -> bytes:
    if len(efi_data) < 0x208:
        raise ValueError("EFI smoke payload is too small to carry a Linux setup header")
    if efi_data[:2] != b"MZ":
        raise ValueError("EFI smoke payload is not a PE/COFF image")

    original_pe_offset = read_u32(efi_data, 0x3C)
    pe_signature = efi_data[original_pe_offset : original_pe_offset + 4]
    if original_pe_offset + 24 >= len(efi_data) or pe_signature != b"PE\0\0":
        raise ValueError("EFI smoke payload has an invalid PE header")

    section_count = read_u16(efi_data, original_pe_offset + 6)
    pointer_to_symbols = read_u32(efi_data, original_pe_offset + 12)
    optional_size = read_u16(efi_data, original_pe_offset + 20)
    optional_offset = original_pe_offset + 24
    optional_magic = read_u16(efi_data, optional_offset)
    if optional_magic == 0x10B:
        data_directory_offset = optional_offset + 96
    elif optional_magic == 0x20B:
        data_directory_offset = optional_offset + 112
    else:
        raise ValueError("EFI smoke payload has an unsupported optional PE header")

    file_alignment = read_u32(efi_data, optional_offset + 36)
    if file_alignment == 0:
        raise ValueError("EFI smoke payload has an invalid file alignment")

    setup_size = align_up(0x400, file_alignment)
    data = bytearray(setup_size + len(efi_data))
    data[setup_size:] = efi_data

    # Linux EFI stubs keep the Linux setup header before the PE/COFF image.
    new_pe_offset = setup_size + original_pe_offset
    data[0:2] = b"MZ"
    write_u32(data, 0x3C, new_pe_offset)
    data[0x202:0x206] = b"HdrS"
    data[0x206:0x208] = (0x0208).to_bytes(2, "little")

    if pointer_to_symbols:
        write_u32(data, new_pe_offset + 12, pointer_to_symbols + setup_size)

    size_of_headers_offset = setup_size + optional_offset + 60
    new_size_of_headers = align_up(
        read_u32(data, size_of_headers_offset) + setup_size,
        file_alignment,
    )
    write_u32(data, size_of_headers_offset, new_size_of_headers)
    write_u32(data, setup_size + optional_offset + 64, 0)

    security_directory_offset = setup_size + data_directory_offset + 4 * 8
    security_file_pointer = read_u32(data, security_directory_offset)
    if security_file_pointer:
        write_u32(data, security_directory_offset, security_file_pointer + setup_size)

    section_table_offset = setup_size + optional_offset + optional_size
    for index in range(section_count):
        section_offset = section_table_offset + index * 40
        raw_pointer_offset = section_offset + 20
        raw_pointer = read_u32(data, raw_pointer_offset)
        if raw_pointer:
            write_u32(data, raw_pointer_offset, raw_pointer + setup_size)

    return bytes(data)


def volume_descriptor_terminator() -> bytes:
    data = bytearray(SECTOR_SIZE)
    data[0] = 255
    data[1:6] = b"CD001"
    data[6] = 1
    return bytes(data)


def make_iso_layout(efi_data: bytes, profile: str) -> tuple[list[dict[str, object]], list[dict[str, object]]]:
    directories: list[dict[str, object]] = [
        {"path": "/", "name": b"\x00", "parent": "/"},
        {"path": "/EFI", "name": b"EFI", "parent": "/"},
    ]
    files: list[dict[str, object]] = []

    if profile == PROFILE_GENERIC:
        directories.append({"path": "/EFI/BOOT", "name": b"BOOT", "parent": "/EFI"})
        files.append(
            {
                "dir": "/EFI/BOOT",
                "name": b"BOOTX64.EFI;1",
                "data": efi_data,
                "eltorito": True,
            }
        )
    elif profile == PROFILE_WINDOWS:
        directories.extend(
            [
                {"path": "/EFI/MICROSOFT", "name": b"MICROSOFT", "parent": "/EFI"},
                {
                    "path": "/EFI/MICROSOFT/BOOT",
                    "name": b"BOOT",
                    "parent": "/EFI/MICROSOFT",
                },
                {"path": "/SOURCES", "name": b"SOURCES", "parent": "/"},
            ]
        )
        files.extend(
            [
                {
                    "dir": "/EFI/MICROSOFT/BOOT",
                    "name": b"BOOTMGFW.EFI;1",
                    "data": efi_data,
                    "eltorito": True,
                },
                {
                    "dir": "/SOURCES",
                    "name": b"BOOT.WIM;1",
                    "data": b"NEXTBOOT SMOKE WINDOWS BOOT WIM\n",
                    "eltorito": False,
                },
            ]
        )
    elif profile == PROFILE_LINUX:
        directories.append({"path": "/BOOT", "name": b"BOOT", "parent": "/"})
        files.extend(
            [
                {
                    "dir": "/BOOT",
                    "name": b"VMLINUZ;1",
                    "data": linux_smoke_kernel(efi_data),
                    "eltorito": True,
                },
                {
                    "dir": "/BOOT",
                    "name": b"INITRD.IMG;1",
                    "data": LINUX_SMOKE_INITRD,
                    "eltorito": False,
                },
            ]
        )
    else:
        raise ValueError(f"unsupported smoke ISO profile: {profile}")

    return directories, files


def write_smoke_iso(output: Path, efi: Path, label: str, profile: str) -> None:
    efi_data = efi.read_bytes()
    directories, files = make_iso_layout(efi_data, profile)

    pvd_lba = 16
    eltorito_record_lba = 17
    terminator_lba = 18
    eltorito_catalog_lba = 19
    le_path_table_lba = 20
    be_path_table_lba = 21
    first_directory_lba = 22
    for offset, directory in enumerate(directories):
        directory["lba"] = first_directory_lba + offset

    next_lba = first_directory_lba + len(directories)
    for file in files:
        data = file["data"]
        assert isinstance(data, bytes)
        file["lba"] = next_lba
        sectors = max(1, math.ceil(len(data) / SECTOR_SIZE))
        file["sectors"] = sectors
        next_lba += sectors

    total_sectors = next_lba
    root_lba = int(directories[0]["lba"])
    eltorito_file = next(file for file in files if file.get("eltorito"))

    directory_by_path = {str(directory["path"]): directory for directory in directories}
    directory_index = {
        str(directory["path"]): index + 1 for index, directory in enumerate(directories)
    }
    path_entries = []
    for directory in directories:
        parent_path = str(directory["parent"])
        path_entries.append(
            (
                directory["name"],
                int(directory["lba"]),
                directory_index[parent_path],
            )
        )

    le_path_table = b"".join(
        path_table_record(name, lba, parent, "little")
        for name, lba, parent in path_entries
    )
    be_path_table = b"".join(
        path_table_record(name, lba, parent, "big")
        for name, lba, parent in path_entries
    )
    path_table_size = len(le_path_table)

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
    image[eltorito_record_lba * SECTOR_SIZE : (eltorito_record_lba + 1) * SECTOR_SIZE] = (
        eltorito_boot_record(eltorito_catalog_lba)
    )
    image[terminator_lba * SECTOR_SIZE : (terminator_lba + 1) * SECTOR_SIZE] = (
        volume_descriptor_terminator()
    )
    image[eltorito_catalog_lba * SECTOR_SIZE : (eltorito_catalog_lba + 1) * SECTOR_SIZE] = (
        eltorito_boot_catalog(int(eltorito_file["lba"]), len(eltorito_file["data"]))
    )
    image[le_path_table_lba * SECTOR_SIZE : (le_path_table_lba + 1) * SECTOR_SIZE] = sector(
        le_path_table
    )
    image[be_path_table_lba * SECTOR_SIZE : (be_path_table_lba + 1) * SECTOR_SIZE] = sector(
        be_path_table
    )
    for directory in directories:
        path = str(directory["path"])
        parent = directory_by_path[str(directory["parent"])]
        records = [
            directory_record(b"\x00", int(directory["lba"]), SECTOR_SIZE, 0x02),
            directory_record(b"\x01", int(parent["lba"]), SECTOR_SIZE, 0x02),
        ]
        for child in directories:
            if child is directory:
                continue
            if child["parent"] == path:
                records.append(directory_record(child["name"], int(child["lba"]), SECTOR_SIZE, 0x02))
        for file in files:
            if file["dir"] == path:
                data = file["data"]
                assert isinstance(data, bytes)
                records.append(directory_record(file["name"], int(file["lba"]), len(data), 0x00))
        start = int(directory["lba"]) * SECTOR_SIZE
        image[start : start + SECTOR_SIZE] = directory_sector(records)

    for file in files:
        data = file["data"]
        assert isinstance(data, bytes)
        file_offset = int(file["lba"]) * SECTOR_SIZE
        image[file_offset : file_offset + len(data)] = data

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(image)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--efi", required=True, type=Path, help="BOOTX64.EFI payload")
    parser.add_argument("--label", default="NEXTSMOKE", help="ISO9660 volume label")
    parser.add_argument(
        "--profile",
        default=PROFILE_GENERIC,
        choices=(PROFILE_GENERIC, PROFILE_WINDOWS, PROFILE_LINUX),
        help="file layout to generate",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.efi.is_file():
        raise SystemExit(f"EFI payload not found: {args.efi}")
    if len(args.label) > 32:
        raise SystemExit("--label must fit in 32 ASCII characters")
    write_smoke_iso(args.output, args.efi, args.label, args.profile)
    print(f"created {args.output} ({os.path.getsize(args.output)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
