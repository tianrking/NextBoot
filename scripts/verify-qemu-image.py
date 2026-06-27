#!/usr/bin/env python3
"""Verify NextBoot raw QEMU disk images.

This checker validates the GPT layout and the small FAT32/exFAT/NTFS volumes created
by scripts/run-qemu.sh.  It intentionally focuses on the structures the
bootloader depends on: partition discovery, filesystem detection, directory
enumeration, file sizes, and physical file extents.
"""

from __future__ import annotations

import argparse
import os
import struct
import sys
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


class Fat32Volume:
    fs_type = "fat32"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing FAT32 boot signature")
        require(self.boot[82:90] == b"FAT32   ", f"{partition.name}: missing FAT32 type marker")

        self.bytes_per_sector = u16(self.boot, 11)
        self.sectors_per_cluster = self.boot[13]
        self.reserved_sectors = u16(self.boot, 14)
        self.num_fats = self.boot[16]
        total16 = u16(self.boot, 19)
        total32 = u32(self.boot, 32)
        self.total_sectors = total16 or total32
        fat16 = u16(self.boot, 22)
        fat32 = u32(self.boot, 36)
        self.fat_size = fat16 or fat32
        self.root_cluster = u32(self.boot, 44)

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: FAT32 sector size mismatch")
        require(self.sectors_per_cluster > 0, f"{partition.name}: invalid FAT32 cluster size")
        require(self.total_sectors <= partition.block_count, f"{partition.name}: FAT32 volume exceeds partition")

        self.fat_lba = partition.start_lba + self.reserved_sectors
        self.data_lba = partition.start_lba + self.reserved_sectors + self.num_fats * self.fat_size

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def cluster_to_lba(self, cluster: int) -> int:
        return self.data_lba + (cluster - 2) * self.sectors_per_cluster

    def read_cluster(self, cluster: int) -> bytes:
        require(cluster >= 2, f"{self.partition.name}: invalid FAT32 cluster {cluster}")
        return self.image.read_blocks(self.cluster_to_lba(cluster), self.sectors_per_cluster)

    def next_cluster(self, cluster: int) -> int:
        offset = (self.fat_lba * self.image.sector_size) + cluster * 4
        return u32(self.image.read_at(offset, 4), 0) & 0x0FFFFFFF

    def cluster_chain(self, start_cluster: int) -> list[int]:
        require(start_cluster >= 2, f"{self.partition.name}: invalid FAT32 chain start")
        out: list[int] = []
        cluster = start_cluster
        while True:
            require(cluster not in out, f"{self.partition.name}: FAT32 cluster loop")
            out.append(cluster)
            nxt = self.next_cluster(cluster)
            if nxt >= FAT_EOC:
                return out
            require(nxt >= 2, f"{self.partition.name}: invalid FAT32 next cluster")
            cluster = nxt

    def read_directory(self, cluster: int) -> list[FileRecord]:
        data = b"".join(self.read_cluster(item) for item in self.cluster_chain(cluster))
        records: list[FileRecord] = []
        lfn_parts: dict[int, str] = {}
        for offset in range(0, len(data), 32):
            entry = data[offset : offset + 32]
            if len(entry) < 32:
                break
            first = entry[0]
            if first == 0:
                break
            if first == 0xE5:
                lfn_parts.clear()
                continue
            attr = entry[11]
            if attr == 0x0F:
                lfn_parts[first & 0x1F] = decode_fat_lfn(entry)
                continue
            if attr & 0x08:
                lfn_parts.clear()
                continue

            if lfn_parts:
                name = "".join(lfn_parts[index] for index in sorted(lfn_parts))
            else:
                name = decode_short_name(entry[:11])
            lfn_parts.clear()

            first_cluster = (u16(entry, 20) << 16) | u16(entry, 26)
            records.append(
                FileRecord(
                    name=name,
                    is_dir=bool(attr & 0x10),
                    size=u32(entry, 28),
                    first_cluster=first_cluster,
                    contiguous=False,
                )
            )
        return records

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty FAT32 lookup")
        cluster = self.root_cluster
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(cluster)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing FAT32 path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                cluster = record.first_cluster
        return record

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        blocks_remaining = ceil_div(record.size, self.image.sector_size)
        virtual_block = 0
        extents: list[FileExtent] = []
        for cluster in self.cluster_chain(record.first_cluster):
            block_count = min(self.cluster_blocks, blocks_remaining)
            append_extent(extents, virtual_block, self.cluster_to_lba(cluster), block_count)
            virtual_block += block_count
            blocks_remaining -= block_count
            if blocks_remaining == 0:
                break
        require(blocks_remaining == 0, f"{record.name}: FAT32 file chain is too short")
        return extents


class ExFatVolume:
    fs_type = "exfat"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[0:3] == b"\xeb\x76\x90", f"{partition.name}: missing exFAT jump")
        require(self.boot[3:11] == b"EXFAT   ", f"{partition.name}: missing exFAT marker")
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing exFAT boot signature")

        self.partition_offset = u64(self.boot, 64)
        self.volume_length = u64(self.boot, 72)
        self.fat_offset = u32(self.boot, 80)
        self.fat_length = u32(self.boot, 84)
        self.cluster_heap_offset = u32(self.boot, 88)
        self.cluster_count = u32(self.boot, 92)
        self.root_cluster = u32(self.boot, 96)
        self.bytes_per_sector = 1 << self.boot[108]
        self.sectors_per_cluster = 1 << self.boot[109]
        self.num_fats = self.boot[110]

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: exFAT sector size mismatch")
        require(self.num_fats == 1, f"{partition.name}: expected one exFAT FAT")
        require(self.partition_offset == partition.start_lba, f"{partition.name}: exFAT partition offset mismatch")
        require(self.volume_length <= partition.block_count, f"{partition.name}: exFAT volume exceeds partition")

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def cluster_to_lba(self, cluster: int) -> int:
        return self.partition.start_lba + self.cluster_heap_offset + (cluster - 2) * self.sectors_per_cluster

    def read_cluster(self, cluster: int) -> bytes:
        require(2 <= cluster < self.cluster_count + 2, f"{self.partition.name}: invalid exFAT cluster {cluster}")
        return self.image.read_blocks(self.cluster_to_lba(cluster), self.sectors_per_cluster)

    def next_cluster(self, cluster: int) -> int:
        offset = (self.partition.start_lba + self.fat_offset) * self.image.sector_size + cluster * 4
        return u32(self.image.read_at(offset, 4), 0)

    def cluster_chain(self, start_cluster: int) -> list[int]:
        require(start_cluster >= 2, f"{self.partition.name}: invalid exFAT chain start")
        out: list[int] = []
        cluster = start_cluster
        while True:
            require(cluster not in out, f"{self.partition.name}: exFAT cluster loop")
            out.append(cluster)
            nxt = self.next_cluster(cluster)
            if nxt >= EXFAT_EOC:
                return out
            require(2 <= nxt < self.cluster_count + 2, f"{self.partition.name}: invalid exFAT next cluster")
            cluster = nxt

    def read_directory(self, cluster: int) -> list[FileRecord]:
        data = b"".join(self.read_cluster(item) for item in self.cluster_chain(cluster))
        records: list[FileRecord] = []
        offset = 0
        while offset + 32 <= len(data):
            entry_type = data[offset]
            if entry_type == 0:
                break
            if entry_type == 0x85:
                secondary_count = data[offset + 1]
                group = data[offset : offset + 32 * (secondary_count + 1)]
                require(len(group) == 32 * (secondary_count + 1), f"{self.partition.name}: truncated exFAT entry set")
                parsed = self.parse_entry_set(group)
                if parsed is not None:
                    records.append(parsed)
                offset += 32 * (secondary_count + 1)
            else:
                offset += 32
        return records

    def parse_entry_set(self, group: bytes) -> FileRecord | None:
        attr = u16(group, 4)
        if attr & 0x0006:
            return None

        first_cluster = 0
        size = 0
        name_length = 0
        name_chars: list[str] = []
        contiguous = False

        for offset in range(32, len(group), 32):
            entry = group[offset : offset + 32]
            if entry[0] == 0xC0:
                contiguous = bool(entry[1] & 0x02)
                name_length = entry[3]
                first_cluster = u32(entry, 20)
                size = u64(entry, 24)
            elif entry[0] == 0xC1:
                remaining = name_length - len(name_chars)
                for index in range(min(15, max(0, remaining))):
                    value = u16(entry, 2 + index * 2)
                    if value == 0:
                        break
                    name_chars.append(chr(value))

        return FileRecord(
            name="".join(name_chars),
            is_dir=bool(attr & 0x0010),
            size=size,
            first_cluster=first_cluster,
            contiguous=contiguous,
        )

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty exFAT lookup")
        cluster = self.root_cluster
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(cluster)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing exFAT path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                cluster = record.first_cluster
        return record

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        blocks_remaining = ceil_div(record.size, self.image.sector_size)
        if record.contiguous:
            return [FileExtent(0, self.cluster_to_lba(record.first_cluster), blocks_remaining)]

        virtual_block = 0
        extents: list[FileExtent] = []
        for cluster in self.cluster_chain(record.first_cluster):
            block_count = min(self.cluster_blocks, blocks_remaining)
            append_extent(extents, virtual_block, self.cluster_to_lba(cluster), block_count)
            virtual_block += block_count
            blocks_remaining -= block_count
            if blocks_remaining == 0:
                break
        require(blocks_remaining == 0, f"{record.name}: exFAT file chain is too short")
        return extents


class NtfsVolume:
    fs_type = "ntfs"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[3:11] == NTFS_OEM_ID, f"{partition.name}: missing NTFS marker")
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing NTFS boot signature")

        self.bytes_per_sector = u16(self.boot, 0x0B)
        self.sectors_per_cluster = self.boot[0x0D]
        self.total_sectors = u64(self.boot, 0x28)
        self.mft_lcn = u64(self.boot, 0x30)
        self.file_record_size = self.decode_record_size(self.boot[0x40])
        self.index_record_size = self.decode_record_size(self.boot[0x44])

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: NTFS sector size mismatch")
        require(self.sectors_per_cluster > 0, f"{partition.name}: invalid NTFS cluster size")
        require(self.total_sectors <= partition.block_count, f"{partition.name}: NTFS volume exceeds partition")
        require(self.file_record_size % image.sector_size == 0, f"{partition.name}: NTFS file record is not sector aligned")
        require(self.index_record_size % image.sector_size == 0, f"{partition.name}: NTFS index record is not sector aligned")

    @property
    def cluster_size(self) -> int:
        return self.bytes_per_sector * self.sectors_per_cluster

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def decode_record_size(self, raw: int) -> int:
        value = raw if raw < 128 else raw - 256
        if value > 0:
            return value * self.bytes_per_sector * self.sectors_per_cluster
        require(value < 0, f"{self.partition.name}: invalid NTFS record size code")
        return 1 << (-value)

    def read_file_record(self, record_number: int) -> bytes:
        offset = (
            self.partition.start_lba * self.image.sector_size
            + self.mft_lcn * self.cluster_size
            + record_number * self.file_record_size
        )
        record = bytearray(self.image.read_at(offset, self.file_record_size))
        require(record[0:4] == b"FILE", f"{self.partition.name}: bad NTFS FILE record {record_number}")
        self.apply_fixup(record)
        return bytes(record)

    def apply_fixup(self, record: bytearray) -> None:
        usa_offset = u16(record, 4)
        usa_count = u16(record, 6)
        sector_count = len(record) // self.bytes_per_sector
        require(usa_count == sector_count + 1, f"{self.partition.name}: invalid NTFS update sequence count")
        require(usa_offset + usa_count * 2 <= len(record), f"{self.partition.name}: NTFS update sequence is out of range")
        sequence = u16(record, usa_offset)
        for sector in range(sector_count):
            tail = (sector + 1) * self.bytes_per_sector - 2
            require(u16(record, tail) == sequence, f"{self.partition.name}: NTFS update sequence mismatch")
            replacement = record[usa_offset + 2 * (sector + 1) : usa_offset + 2 * (sector + 2)]
            record[tail : tail + 2] = replacement

    def attributes(self, record: bytes) -> list[tuple[int, bytes]]:
        attrs_offset = u16(record, 0x14)
        require(attrs_offset < len(record), f"{self.partition.name}: NTFS attribute offset is out of range")
        out: list[tuple[int, bytes]] = []
        offset = attrs_offset
        while offset + 8 <= len(record):
            attr_type = u32(record, offset)
            if attr_type == NTFS_ATTR_TYPE_END:
                break
            attr_len = u32(record, offset + 4)
            require(attr_len > 0, f"{self.partition.name}: zero-length NTFS attribute")
            require(offset + attr_len <= len(record), f"{self.partition.name}: NTFS attribute exceeds record")
            out.append((attr_type, record[offset : offset + attr_len]))
            offset += attr_len
        return out

    def resident_value(self, attr: bytes) -> bytes:
        require(attr[8] == 0, f"{self.partition.name}: expected resident NTFS attribute")
        value_len = u32(attr, 0x10)
        value_offset = u16(attr, 0x14)
        require(value_offset + value_len <= len(attr), f"{self.partition.name}: NTFS resident value exceeds attribute")
        return attr[value_offset : value_offset + value_len]

    def parse_index_entries(self, data: bytes) -> list[FileRecord]:
        records: list[FileRecord] = []
        offset = 0
        while offset + 16 <= len(data):
            entry_len = u16(data, offset + 8)
            stream_len = u16(data, offset + 10)
            flags = u16(data, offset + 12)
            require(entry_len > 0, f"{self.partition.name}: zero-length NTFS index entry")
            require(offset + entry_len <= len(data), f"{self.partition.name}: NTFS index entry exceeds buffer")
            if flags & NTFS_INDEX_ENTRY_LAST:
                break
            stream_start = offset + 16
            stream_end = stream_start + stream_len
            require(stream_end <= offset + entry_len, f"{self.partition.name}: NTFS index stream exceeds entry")
            parsed = self.parse_file_name_entry(data[offset : offset + 6], data[stream_start:stream_end])
            if parsed is not None:
                records.append(parsed)
            offset += entry_len
        return records

    def parse_file_name_entry(self, record_ref: bytes, stream: bytes) -> FileRecord | None:
        if len(stream) < 66:
            return None
        namespace = stream[65]
        if namespace == 2:
            return None
        allocated_size = u64(stream, 40)
        real_size = u64(stream, 48)
        raw_flags = u32(stream, 56)
        name_len = stream[64]
        name_bytes = name_len * 2
        require(66 + name_bytes <= len(stream), f"{self.partition.name}: NTFS filename exceeds entry")
        name = stream[66 : 66 + name_bytes].decode("utf-16le", errors="strict")
        is_dir = bool(raw_flags & NTFS_FILE_ATTRIBUTE_DIRECTORY)
        return FileRecord(
            name=name,
            is_dir=is_dir,
            size=allocated_size if is_dir else real_size,
            first_cluster=int.from_bytes(record_ref + b"\x00\x00", "little"),
            contiguous=False,
        )

    def read_directory(self, record_number: int) -> list[FileRecord]:
        record = self.read_file_record(record_number)
        flags = u16(record, 0x16)
        require(flags & 0x0002, f"{self.partition.name}: NTFS record {record_number} is not a directory")
        for attr_type, attr in self.attributes(record):
            if attr_type != NTFS_ATTR_TYPE_INDEX_ROOT:
                continue
            value = self.resident_value(attr)
            require(len(value) >= 32, f"{self.partition.name}: NTFS index root is too small")
            index_header = 16
            entries_offset = u32(value, index_header)
            total_size = u32(value, index_header + 4)
            start = index_header + entries_offset
            end = min(index_header + total_size, len(value))
            require(start <= end, f"{self.partition.name}: invalid NTFS index range")
            return self.parse_index_entries(value[start:end])
        raise VerifyError(f"{self.partition.name}: NTFS directory record {record_number} has no index root")

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty NTFS lookup")
        record_number = 5
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(record_number)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing NTFS path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                record_number = record.first_cluster
        return record

    def parse_data_runs(self, data: bytes) -> list[tuple[int, int, int | None]]:
        runs: list[tuple[int, int, int | None]] = []
        offset = 0
        current_vcn = 0
        current_lcn = 0
        while offset < len(data):
            header = data[offset]
            offset += 1
            if header == 0:
                break
            len_size = header & 0x0F
            off_size = header >> 4
            require(len_size > 0 and len_size <= 8 and off_size <= 8, f"{self.partition.name}: invalid NTFS run header")
            require(offset + len_size + off_size <= len(data), f"{self.partition.name}: truncated NTFS run")
            cluster_count = int.from_bytes(data[offset : offset + len_size], "little")
            offset += len_size
            require(cluster_count > 0, f"{self.partition.name}: zero-length NTFS run")
            if off_size:
                delta = int.from_bytes(data[offset : offset + off_size], "little", signed=True)
                offset += off_size
                current_lcn += delta
                require(current_lcn >= 0, f"{self.partition.name}: negative NTFS run LCN")
                lcn: int | None = current_lcn
            else:
                lcn = None
            runs.append((current_vcn, cluster_count, lcn))
            current_vcn += cluster_count
        return runs

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        file_record = self.read_file_record(record.first_cluster)
        data_attrs = [attr for attr_type, attr in self.attributes(file_record) if attr_type == NTFS_ATTR_TYPE_DATA]
        require(data_attrs, f"{record.name}: NTFS file has no data attribute")
        require(len(data_attrs) == 1, f"{record.name}: verifier supports one NTFS data attribute")
        attr = data_attrs[0]
        require(attr[8] != 0, f"{record.name}: verifier expects non-resident NTFS data")
        real_size = u64(attr, 0x30)
        require(real_size == record.size, f"{record.name}: NTFS data size mismatch")
        runlist_offset = u16(attr, 0x20)
        require(runlist_offset < len(attr), f"{record.name}: NTFS runlist offset is out of range")
        runs = self.parse_data_runs(attr[runlist_offset:])
        remaining_blocks = ceil_div(record.size, self.image.sector_size)
        extents: list[FileExtent] = []
        for virtual_cluster, cluster_count, lcn in runs:
            require(lcn is not None, f"{record.name}: sparse NTFS runs are not expected")
            run_blocks = cluster_count * self.cluster_blocks
            block_count = min(run_blocks, remaining_blocks)
            if block_count == 0:
                break
            append_extent(
                extents,
                virtual_cluster * self.cluster_blocks,
                self.partition.start_lba + lcn * self.cluster_blocks,
                block_count,
            )
            remaining_blocks -= block_count
            if remaining_blocks == 0:
                break
        require(remaining_blocks == 0, f"{record.name}: NTFS file runs are too short")
        return extents


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


def make_volume(image: DiskImage, partition: Partition, expected_fs: str):
    if expected_fs == "fat32":
        return Fat32Volume(image, partition)
    if expected_fs == "exfat":
        return ExFatVolume(image, partition)
    if expected_fs == "ntfs":
        return NtfsVolume(image, partition)
    raise VerifyError(f"unsupported expected filesystem {expected_fs}")


def read_record_region(image: DiskImage, extents: list[FileExtent], offset: int, length: int) -> bytes:
    if length == 0:
        return b""
    sector_size = image.sector_size
    end = offset + length
    chunks: list[bytes] = []
    for extent in extents:
        extent_start = extent.virtual_block_start * sector_size
        extent_end = extent_start + extent.block_count * sector_size
        if end <= extent_start or offset >= extent_end:
            continue
        read_start = max(offset, extent_start)
        read_end = min(end, extent_end)
        disk_offset = extent.physical_lba * sector_size + (read_start - extent_start)
        chunks.append(image.read_at(disk_offset, read_end - read_start))
    data = b"".join(chunks)
    require(len(data) == length, "file extent read did not cover requested range")
    return data


def compare_source_sample(image: DiskImage, extents: list[FileExtent], record: FileRecord, source: str) -> None:
    expected_size = os.path.getsize(source)
    require(record.size == expected_size, f"{record.name}: size mismatch ({record.size} != {expected_size})")
    if expected_size == 0:
        return

    sample_len = min(expected_size, 65536)
    with open(source, "rb") as src:
        expected_head = src.read(sample_len)
    actual_head = read_record_region(image, extents, 0, sample_len)
    require(actual_head == expected_head, f"{record.name}: leading data sample mismatch")

    if expected_size > sample_len:
        with open(source, "rb") as src:
            src.seek(expected_size - sample_len)
            expected_tail = src.read(sample_len)
        actual_tail = read_record_region(image, extents, expected_size - sample_len, sample_len)
        require(actual_tail == expected_tail, f"{record.name}: trailing data sample mismatch")


def assert_extents_inside_partition(partition: Partition, record: FileRecord, extents: list[FileExtent]) -> None:
    for extent in extents:
        require(extent.physical_lba >= partition.start_lba, f"{record.name}: extent starts before partition")
        require(
            extent.physical_lba + extent.block_count <= partition.end_lba + 1,
            f"{record.name}: extent ends after partition",
        )


def verify_efi(volume, image: DiskImage, source: str | None) -> None:
    efi = volume.lookup("/EFI/BOOT/BOOTX64.EFI")
    require(not efi.is_dir, "BOOTX64.EFI is a directory")
    extents = volume.file_extents(efi)
    assert_extents_inside_partition(volume.partition, efi, extents)
    if source:
        compare_source_sample(image, extents, efi, source)


def verify_iso_directory(volume, image: DiskImage, sources: list[str]) -> None:
    iso = volume.lookup("/ISO")
    require(iso.is_dir, "/ISO is not a directory")
    expected_by_name = {os.path.basename(path).lower(): path for path in sources}
    entries = volume.read_directory(iso.first_cluster)
    files = {entry.name.lower(): entry for entry in entries if not entry.is_dir}

    require(
        set(files) == set(expected_by_name),
        f"/ISO contents mismatch: got {sorted(files)}, expected {sorted(expected_by_name)}",
    )

    for lower_name, source in expected_by_name.items():
        record = files[lower_name]
        extents = volume.file_extents(record)
        assert_extents_inside_partition(volume.partition, record, extents)
        compare_source_sample(image, extents, record, source)


def find_partition(partitions: list[Partition], name: str) -> Partition:
    for partition in partitions:
        if partition.name == name:
            return partition
    raise VerifyError(f"missing GPT partition {name}")


def verify_layout(args: argparse.Namespace) -> None:
    image = DiskImage(args.disk_image, args.sector_size)
    try:
        partitions = parse_gpt(image)
        if args.layout == "single":
            require(len(partitions) == 1, f"single layout expected 1 partition, got {len(partitions)}")
            part = find_partition(partitions, "NEXBOOT")
            require(part.type_guid == ESP_GUID, "single partition is not an ESP")
            volume = make_volume(image, part, "fat32")
            verify_efi(volume, image, args.efi_file)
            verify_iso_directory(volume, image, args.image)
            print(f"verified single GPT/FAT32 layout: {part.name}")
        else:
            require(len(partitions) == 2, f"split layout expected 2 partitions, got {len(partitions)}")
            esp = find_partition(partitions, "NEXBOOT_EFI")
            data = find_partition(partitions, "NEXBOOT_DATA")
            require(esp.type_guid == ESP_GUID, "split ESP partition has wrong type GUID")
            require(data.type_guid == MS_BASIC_GUID, "split Data partition has wrong type GUID")
            esp_volume = make_volume(image, esp, "fat32")
            data_volume = make_volume(image, data, args.data_fs)
            verify_efi(esp_volume, image, args.efi_file)
            verify_iso_directory(data_volume, image, args.image)
            print(f"verified split GPT layout: {esp.name}=FAT32 {data.name}={args.data_fs}")
        print(f"verified {len(args.image)} /ISO image file(s) on {args.sector_size} byte sectors")
    finally:
        image.close()


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disk-image", required=True, help="raw disk image to verify")
    parser.add_argument("--sector-size", required=True, type=int, choices=(512, 4096))
    parser.add_argument("--layout", required=True, choices=("single", "split"))
    parser.add_argument("--data-fs", default="exfat", choices=("exfat", "fat32", "ntfs"))
    parser.add_argument("--efi-file", help="expected BOOTX64.EFI source file")
    parser.add_argument("--image", action="append", default=[], help="expected /ISO image file; repeatable")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        verify_layout(args)
    except VerifyError as exc:
        print(f"verify-qemu-image: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
