#!/usr/bin/env python3
"""Grow a NextBoot release image after it has been written to larger media."""

from __future__ import annotations

import argparse
import os
import struct
import zlib
from dataclasses import dataclass


GPT_SIGNATURE = b"EFI PART"
GPT_ENTRY_COUNT_MAX = 128
NEXBOOT_DATA = "NEXBOOT_DATA"
NEXBOOT_EFI = "NEXBOOT_EFI"
EXFAT_BOOT_REGION_SECTORS = 12
EXFAT_BOOT_CHECKSUM_SECTOR = 11


class GrowError(Exception):
    pass


@dataclass
class GptPartition:
    index: int
    start_lba: int
    end_lba: int
    name: str

    @property
    def block_count(self) -> int:
        return self.end_lba - self.start_lba + 1


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GrowError(message)


def u32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def u64(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<Q", data, offset)[0]


def put_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def put_u64(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<Q", data, offset, value)


def ceil_div(value: int, divisor: int) -> int:
    return (value + divisor - 1) // divisor


def decode_gpt_name(raw: bytes) -> str:
    return raw.decode("utf-16le", errors="ignore").rstrip("\x00")


def validate_header(header: bytes, sector_size: int, expected_lba: int) -> None:
    require(header[0:8] == GPT_SIGNATURE, "missing GPT signature")
    header_size = u32(header, 12)
    require(92 <= header_size <= sector_size, "invalid GPT header size")
    require(u64(header, 24) == expected_lba, "GPT header is at unexpected LBA")
    expected_crc = u32(header, 16)
    scratch = bytearray(header[:header_size])
    put_u32(scratch, 16, 0)
    require((zlib.crc32(scratch) & 0xFFFFFFFF) == expected_crc, "GPT header CRC mismatch")


def header_with_crc(header: bytearray) -> bytearray:
    header_size = u32(header, 12)
    put_u32(header, 16, 0)
    put_u32(header, 16, zlib.crc32(header[:header_size]) & 0xFFFFFFFF)
    return header


def parse_partitions(entries: bytes, entry_count: int, entry_size: int) -> list[GptPartition]:
    partitions: list[GptPartition] = []
    for index in range(entry_count):
        offset = index * entry_size
        entry = entries[offset : offset + entry_size]
        if entry[0:16] == bytes(16):
            continue
        start_lba = u64(entry, 32)
        end_lba = u64(entry, 40)
        if start_lba == 0 or end_lba < start_lba:
            continue
        partitions.append(
            GptPartition(
                index=index,
                start_lba=start_lba,
                end_lba=end_lba,
                name=decode_gpt_name(entry[56:128]),
            )
        )
    return partitions


def read_at(handle, offset: int, size: int) -> bytes:
    handle.seek(offset)
    data = handle.read(size)
    require(len(data) == size, "short read")
    return data


def write_at(handle, offset: int, data: bytes | bytearray) -> None:
    handle.seek(offset)
    handle.write(data)


def update_exfat_boot_checksum(region: bytearray, sector_size: int) -> None:
    checksum_offset = EXFAT_BOOT_CHECKSUM_SECTOR * sector_size
    checksum = 0
    for offset, byte in enumerate(region[:checksum_offset]):
        if offset in (106, 107, 112):
            continue
        checksum = ((checksum >> 1) | ((checksum & 1) << 31)) & 0xFFFFFFFF
        checksum = (checksum + byte) & 0xFFFFFFFF
    region[checksum_offset : checksum_offset + sector_size] = (
        struct.pack("<I", checksum) * (sector_size // 4)
    )


def exfat_cluster_offset(data: GptPartition, sector_size: int, cluster_heap_offset: int,
                         sectors_per_cluster: int, cluster: int) -> int:
    require(cluster >= 2, "invalid exFAT cluster number")
    return data.start_lba * sector_size + (
        cluster_heap_offset + (cluster - 2) * sectors_per_cluster
    ) * sector_size


def plan_exfat_bitmap_length(handle, sector_size: int, data: GptPartition, boot: bytes,
                             old_cluster_count: int, new_cluster_count: int) -> tuple[int, bytes]:
    fat_offset = u32(boot, 80)
    cluster_heap_offset = u32(boot, 88)
    sectors_per_cluster = 1 << boot[109]
    root_cluster = u32(boot, 96)
    cluster_size = sectors_per_cluster * sector_size
    fat_base = data.start_lba * sector_size + fat_offset * sector_size
    fat_length = u32(boot, 84)

    def fat_next(cluster: int) -> int:
        require(cluster * 4 + 4 <= fat_length * sector_size, "exFAT FAT entry is outside FAT")
        return u32(read_at(handle, fat_base + cluster * 4, 4), 0)

    def chain(first_cluster: int) -> list[int]:
        result = []
        current = first_cluster
        while True:
            require(current >= 2 and current < old_cluster_count + 2, "invalid exFAT cluster chain")
            require(current not in result, "cyclic exFAT cluster chain")
            result.append(current)
            following = fat_next(current)
            if following >= 0xFFFF_FFF8:
                return result
            current = following

    bitmap_entry_offset = None
    bitmap_first_cluster = None
    for cluster in chain(root_cluster):
        base = exfat_cluster_offset(data, sector_size, cluster_heap_offset, sectors_per_cluster, cluster)
        directory = read_at(handle, base, cluster_size)
        for offset in range(0, cluster_size, 32):
            entry_type = directory[offset]
            if entry_type == 0:
                break
            if entry_type == 0x81:
                require(bitmap_entry_offset is None, "multiple exFAT allocation bitmaps")
                bitmap_entry_offset = base + offset
                bitmap_first_cluster = u32(directory, offset + 20)
        if bitmap_entry_offset is not None:
            break

    require(bitmap_entry_offset is not None and bitmap_first_cluster is not None,
            "missing exFAT allocation bitmap")
    old_size = u64(read_at(handle, bitmap_entry_offset, 32), 24)
    exact_old_size = ceil_div(old_cluster_count, 8)
    # Older release candidates wrote a capacity-sized bitmap length.  It is
    # safe to grow them too, but new media always uses the exact length.
    require(old_size >= exact_old_size, "exFAT allocation bitmap is too small")
    new_size = ceil_div(new_cluster_count, 8)
    storage_capacity = len(chain(bitmap_first_cluster)) * cluster_size
    require(new_size <= storage_capacity, "exFAT allocation bitmap cannot cover grown volume")
    entry = bytearray(read_at(handle, bitmap_entry_offset, 32))
    put_u64(entry, 24, new_size)
    return bitmap_entry_offset, bytes(entry)


def plan_exfat_boot(handle, sector_size: int, data: GptPartition,
                    new_blocks: int) -> tuple[int, bytearray, tuple[int, bytes]]:
    boot_offset = data.start_lba * sector_size
    boot_region = bytearray(read_at(handle, boot_offset, EXFAT_BOOT_REGION_SECTORS * sector_size))
    boot = boot_region[:sector_size]
    require(boot[0:3] == b"\xeb\x76\x90" and boot[3:11] == b"EXFAT   ", "NEXTDATA is not exFAT")
    require(u64(boot, 64) == data.start_lba, "exFAT partition offset does not match GPT")
    bytes_per_sector = 1 << boot[108]
    require(bytes_per_sector == sector_size, "exFAT sector size does not match media")
    sectors_per_cluster = 1 << boot[109]
    fat_length = u32(boot, 84)
    cluster_heap_offset = u32(boot, 88)
    require(fat_length > 0 and cluster_heap_offset > 24, "invalid growable exFAT geometry")

    fat_cluster_capacity = fat_length * sector_size // 4 - 2
    old_blocks = u64(boot, 72)
    old_cluster_count = u32(boot, 92)
    require(new_blocks >= old_blocks, "refusing to shrink existing exFAT volume")
    require(old_blocks > cluster_heap_offset, "invalid existing exFAT volume length")
    capacity = min(fat_cluster_capacity, 0xFFFF_FFFD)
    require((old_blocks - cluster_heap_offset) // sectors_per_cluster <= capacity,
            "existing exFAT clusters exceed FAT capacity")
    capacity_blocks = cluster_heap_offset + capacity * sectors_per_cluster
    grown_blocks = min(new_blocks, max(capacity_blocks, old_blocks))
    new_cluster_count = (grown_blocks - cluster_heap_offset) // sectors_per_cluster
    require(new_cluster_count >= 16, "expanded exFAT volume would be too small")

    put_u64(boot, 72, grown_blocks)
    put_u32(boot, 92, new_cluster_count)
    boot_region[:sector_size] = boot
    update_exfat_boot_checksum(boot_region, sector_size)
    bitmap_update = plan_exfat_bitmap_length(
        handle, sector_size, data, boot, old_cluster_count, new_cluster_count
    )
    return grown_blocks, boot_region, bitmap_update


def grow(path: str, sector_size: int, media_size_bytes: int | None = None) -> str:
    size = media_size_bytes if media_size_bytes is not None else os.path.getsize(path)
    require(size % sector_size == 0, "image size is not sector aligned")
    total_blocks = size // sector_size
    require(total_blocks > 128, "image is too small")
    last_lba = total_blocks - 1

    with open(path, "r+b") as handle:
        mbr = bytearray(read_at(handle, 0, sector_size))
        require(mbr[510:512] == b"\x55\xaa", "missing protective MBR signature")
        require(mbr[0x1BE + 4] == 0xEE, "not a GPT protective MBR")
        header = bytearray(read_at(handle, sector_size, sector_size))
        validate_header(header, sector_size, 1)

        entry_lba = u64(header, 72)
        entry_count = u32(header, 80)
        entry_size = u32(header, 84)
        require(0 < entry_count <= GPT_ENTRY_COUNT_MAX, "unsupported GPT entry count")
        require(entry_size >= 128 and entry_size % 8 == 0, "unsupported GPT entry size")
        entry_bytes_len = entry_count * entry_size
        entry_array_sectors = ceil_div(entry_bytes_len, sector_size)
        entries = bytearray(read_at(handle, entry_lba * sector_size, entry_bytes_len))
        require((zlib.crc32(entries) & 0xFFFFFFFF) == u32(header, 88), "GPT entry CRC mismatch")

        partitions = parse_partitions(entries, entry_count, entry_size)
        require(any(part.name == NEXBOOT_EFI for part in partitions), "missing NEXBOOT_EFI")
        data = next((part for part in partitions if part.name == NEXBOOT_DATA), None)
        require(data is not None, "missing NEXBOOT_DATA")
        require(
            all(part.start_lba <= data.start_lba for part in partitions),
            "NEXBOOT_DATA is not physically last partition",
        )

        backup_entries_lba = last_lba - entry_array_sectors
        new_last_usable = backup_entries_lba - 1
        require(new_last_usable > data.start_lba, "no room for expanded data partition")
        available_blocks = new_last_usable - data.start_lba + 1
        grown_blocks, boot_region, bitmap_update = plan_exfat_boot(
            handle, sector_size, data, available_blocks
        )
        new_data_end = data.start_lba + grown_blocks - 1
        require(new_data_end >= data.end_lba, "refusing to shrink existing data partition")
        if new_data_end == data.end_lba and u64(header, 32) == last_lba:
            return "release media is already grown; no changes made"

        entry_offset = data.index * entry_size
        put_u64(entries, entry_offset + 40, new_data_end)
        entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

        # Every geometry check above is complete before the first write.
        boot_offset = data.start_lba * sector_size
        write_at(handle, bitmap_update[0], bitmap_update[1])
        write_at(handle, boot_offset, boot_region)
        write_at(handle, boot_offset + 12 * sector_size, boot_region)

        put_u32(mbr, 0x1BE + 12, min(total_blocks - 1, 0xFFFF_FFFF))
        write_at(handle, 0, mbr)
        write_at(handle, entry_lba * sector_size, entries)
        write_at(handle, backup_entries_lba * sector_size, entries)

        put_u64(header, 32, last_lba)
        put_u64(header, 48, new_last_usable)
        put_u32(header, 88, entries_crc)
        write_at(handle, sector_size, header_with_crc(header))

        backup_header = bytearray(header)
        put_u64(backup_header, 24, last_lba)
        put_u64(backup_header, 32, 1)
        put_u64(backup_header, 72, backup_entries_lba)
        write_at(handle, last_lba * sector_size, header_with_crc(backup_header))
        handle.flush()

    gib = grown_blocks * sector_size / 1024 / 1024 / 1024
    return f"grew NEXTDATA to {gib:.2f} GiB ({grown_blocks} sectors)"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disk-image", required=True, help="raw NextBoot disk image")
    parser.add_argument("--sector-size", type=int, default=512, choices=(512, 4096))
    parser.add_argument("--target-size-mib", type=int, help="resize file before growing")
    parser.add_argument(
        "--media-size-bytes",
        type=int,
        help="explicit target media size for block devices that do not report st_size",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.target_size_mib is not None:
            require(os.path.isfile(args.disk_image), "--target-size-mib requires a regular image file")
            target_bytes = args.target_size_mib * 1024 * 1024
            require(target_bytes >= os.path.getsize(args.disk_image), "refusing to shrink image file")
            with open(args.disk_image, "ab") as handle:
                handle.truncate(target_bytes)
        print(grow(args.disk_image, args.sector_size, args.media_size_bytes))
    except GrowError as error:
        print(f"grow-release-media: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
