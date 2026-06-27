"""Shared structures and low-level disk helpers for QEMU image verification."""

from __future__ import annotations

import os
import struct
import zlib
from dataclasses import dataclass
from typing import BinaryIO

ESP_GUID = bytes.fromhex("28732ac11ff8d211ba4b00a0c93ec93b")
MS_BASIC_GUID = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")
FAT_EOC = 0x0FFFFFF8
EXFAT_EOC = 0xFFFFFFF8
NTFS_OEM_ID = b"NTFS    "
NTFS_ATTR_TYPE_DATA = 0x80
NTFS_ATTR_TYPE_INDEX_ROOT = 0x90
NTFS_ATTR_TYPE_END = 0xFFFFFFFF
NTFS_FILE_ATTRIBUTE_DIRECTORY = 0x10000000
NTFS_INDEX_ENTRY_LAST = 0x0002


class VerifyError(Exception):
    pass


@dataclass
class Partition:
    number: int
    type_guid: bytes
    start_lba: int
    end_lba: int
    name: str

    @property
    def block_count(self) -> int:
        return self.end_lba - self.start_lba + 1


@dataclass
class FileRecord:
    name: str
    is_dir: bool
    size: int
    first_cluster: int
    contiguous: bool


@dataclass
class FileExtent:
    virtual_block_start: int
    physical_lba: int
    block_count: int


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerifyError(message)


def u16(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def u32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def u64(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<Q", data, offset)[0]


def decode_gpt_name(raw: bytes) -> str:
    return raw.decode("utf-16le", errors="ignore").rstrip("\x00")


def decode_short_name(name11: bytes) -> str:
    base = name11[:8].decode("ascii", errors="ignore").rstrip()
    ext = name11[8:11].decode("ascii", errors="ignore").rstrip()
    return base if not ext else f"{base}.{ext}"


def decode_fat_lfn(entry: bytes) -> str:
    chars: list[str] = []
    for offset in (1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30):
        value = u16(entry, offset)
        if value in (0x0000, 0xFFFF):
            continue
        chars.append(chr(value))
    return "".join(chars)


def ceil_div(value: int, divisor: int) -> int:
    return (value + divisor - 1) // divisor


class DiskImage:
    def __init__(self, path: str, sector_size: int):
        self.path = path
        self.sector_size = sector_size
        self.size = os.path.getsize(path)
        require(self.size > 0, f"{path} is empty")
        require(
            self.size % sector_size == 0,
            f"{path} size is not aligned to {sector_size} byte sectors",
        )
        self.total_blocks = self.size // sector_size
        self.file: BinaryIO = open(path, "rb")

    def close(self) -> None:
        self.file.close()

    def read_at(self, offset: int, size: int) -> bytes:
        require(offset >= 0 and size >= 0, "negative read")
        require(offset + size <= self.size, "read beyond image end")
        self.file.seek(offset)
        data = self.file.read(size)
        require(len(data) == size, "short read")
        return data

    def read_blocks(self, lba: int, count: int = 1) -> bytes:
        return self.read_at(lba * self.sector_size, count * self.sector_size)


def validate_gpt_header(header: bytes, expected_lba: int | None = None) -> None:
    require(header[0:8] == b"EFI PART", "missing GPT signature")
    header_size = u32(header, 12)
    require(92 <= header_size <= len(header), "invalid GPT header size")
    if expected_lba is not None:
        require(u64(header, 24) == expected_lba, "GPT header is at unexpected LBA")

    expected_crc = u32(header, 16)
    scratch = bytearray(header[:header_size])
    scratch[16:20] = b"\x00\x00\x00\x00"
    actual_crc = zlib.crc32(scratch) & 0xFFFFFFFF
    require(actual_crc == expected_crc, "GPT header CRC mismatch")


def parse_gpt(image: DiskImage) -> list[Partition]:
    mbr = image.read_blocks(0)
    require(mbr[510:512] == b"\x55\xaa", "missing protective MBR signature")
    require(mbr[0x1BE + 4] == 0xEE, "protective MBR partition is not type 0xEE")

    header = image.read_blocks(1)
    validate_gpt_header(header, expected_lba=1)

    backup_lba = u64(header, 32)
    require(backup_lba == image.total_blocks - 1, "backup GPT header is not at last LBA")
    backup_header = image.read_blocks(backup_lba)
    validate_gpt_header(backup_header, expected_lba=backup_lba)

    entry_lba = u64(header, 72)
    entry_count = u32(header, 80)
    entry_size = u32(header, 84)
    entry_crc = u32(header, 88)
    require(entry_size >= 128 and entry_size % 8 == 0, "invalid GPT entry size")
    require(0 < entry_count <= 128, "unexpected GPT partition entry count")

    entry_bytes = entry_count * entry_size
    entries = image.read_at(entry_lba * image.sector_size, entry_bytes)
    require((zlib.crc32(entries) & 0xFFFFFFFF) == entry_crc, "GPT entry CRC mismatch")

    partitions: list[Partition] = []
    first_usable = u64(header, 40)
    last_usable = u64(header, 48)
    for index in range(entry_count):
        offset = index * entry_size
        entry = entries[offset : offset + entry_size]
        type_guid = entry[0:16]
        if type_guid == bytes(16):
            continue
        start_lba = u64(entry, 32)
        end_lba = u64(entry, 40)
        name = decode_gpt_name(entry[56:128])
        require(first_usable <= start_lba <= end_lba <= last_usable, f"partition {index + 1} is outside GPT usable range")
        partitions.append(
            Partition(
                number=index + 1,
                type_guid=type_guid,
                start_lba=start_lba,
                end_lba=end_lba,
                name=name,
            )
        )

    require(partitions, "GPT has no usable partitions")
    return partitions
def append_extent(extents: list[FileExtent], virtual_block: int, physical_lba: int, block_count: int) -> None:
    if block_count == 0:
        return
    if extents:
        last = extents[-1]
        if (
            last.virtual_block_start + last.block_count == virtual_block
            and last.physical_lba + last.block_count == physical_lba
        ):
            last.block_count += block_count
            return
    extents.append(FileExtent(virtual_block, physical_lba, block_count))
