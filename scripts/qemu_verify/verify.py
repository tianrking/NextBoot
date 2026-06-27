"""High-level QEMU image layout verification."""

from __future__ import annotations

import argparse
import os

from .common import (
    ESP_GUID,
    MS_BASIC_GUID,
    DiskImage,
    FileExtent,
    FileRecord,
    Partition,
    VerifyError,
    parse_gpt,
    require,
)
from .exfat import ExFatVolume
from .ext4 import Ext4Volume
from .fat32 import Fat32Volume
from .ntfs import NtfsVolume
from .udf import UdfVolume

def make_volume(image: DiskImage, partition: Partition, expected_fs: str):
    if expected_fs == "fat32":
        return Fat32Volume(image, partition)
    if expected_fs == "exfat":
        return ExFatVolume(image, partition)
    if expected_fs in ("ext2", "ext3", "ext4"):
        return Ext4Volume(image, partition)
    if expected_fs == "ntfs":
        return NtfsVolume(image, partition)
    if expected_fs == "udf":
        return UdfVolume(image, partition)
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
