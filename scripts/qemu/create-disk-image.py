#!/usr/bin/env python3
import math
import lzma
import os
import struct
import sys
import time
import uuid
import zlib

from disk_image.exfat import write_exfat_volume
from disk_image.ext4 import write_ext4_volume
from disk_image.fat32 import write_fat32_volume
from disk_image.ntfs import write_ntfs_volume
from disk_image.udf import write_udf_volume
from disk_image.xfs import write_xfs_volume
from disk_image.smoke import (
    make_smoke_auto_memdisk_files,
    make_smoke_linux_plugin_files,
    make_smoke_windows_wimboot_files,
)

path = sys.argv[1]
size_mb = int(sys.argv[2])
sector_size = int(sys.argv[3])
layout = sys.argv[4]
data_fs = sys.argv[5]
efi_file = sys.argv[6]
smoke_linux_plugins = sys.argv[7] == "1"
smoke_windows_wimboot = sys.argv[8] == "1"
smoke_vlnk_iso = sys.argv[9] == "1"
smoke_vlnk_file = sys.argv[10]
smoke_helper_file = sys.argv[11]
smoke_auto_memdisk = sys.argv[12] == "1"
image_files = sys.argv[13:]
if sector_size not in (512, 4096):
    raise SystemExit("sector size must be 512 or 4096")
if layout not in ("single", "split"):
    raise SystemExit("layout must be single or split")
if data_fs not in ("exfat", "ext2", "ext3", "ext4", "fat32", "ntfs", "udf", "xfs"):
    raise SystemExit("data filesystem must be exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs")
total_bytes = size_mb * 1024 * 1024
if total_bytes % sector_size != 0:
    raise SystemExit("disk size must be aligned to the sector size")
total_sectors = total_bytes // sector_size
last_lba = total_sectors - 1
entry_count = 128
entry_size = 128
entry_array_sectors = math.ceil(entry_count * entry_size / sector_size)
primary_entries_lba = 2
first_usable_lba = primary_entries_lba + entry_array_sectors
backup_entries_lba = last_lba - entry_array_sectors
last_usable_lba = backup_entries_lba - 1
alignment_lba = max(1, 1024 * 1024 // sector_size)

if first_usable_lba >= last_usable_lba:
    raise SystemExit("disk image is too small")

def align_up(value, alignment):
    return ((value + alignment - 1) // alignment) * alignment

def mib_to_sectors(mib):
    return mib * 1024 * 1024 // sector_size

def short_name_checksum(name11):
    checksum = 0
    for byte in name11:
        checksum = (((checksum & 1) << 7) + (checksum >> 1) + byte) & 0xFF
    return checksum

def sanitize_short_component(text):
    allowed = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789$%'-_@~`!(){}^#&"
    out = bytearray()
    for ch in text.upper().encode("ascii", "ignore"):
        out.append(ch if ch in allowed else ord("_"))
    return bytes(out)

def split_name(name):
    if "." in name and not name.startswith("."):
        base, ext = name.rsplit(".", 1)
    else:
        base, ext = name, ""
    return base, ext

def split_virtual_path(path):
    parts = [part for part in path.replace("\\", "/").split("/") if part]
    if not parts:
        raise SystemExit(f"invalid virtual path: {path}")
    return parts


extra_files = []
if smoke_auto_memdisk:
    extra_files.extend(make_smoke_auto_memdisk_files(image_files))
if smoke_linux_plugins:
    extra_files.extend(make_smoke_linux_plugin_files(image_files))
if smoke_windows_wimboot:
    extra_files.extend(make_smoke_windows_wimboot_files(smoke_helper_file))

def make_short_name(name, used):
    base, ext = split_name(name)
    clean_base = sanitize_short_component(base) or b"FILE"
    clean_ext = sanitize_short_component(ext)

    candidate = clean_base[:8].ljust(8, b" ") + clean_ext[:3].ljust(3, b" ")
    simple_name = clean_base.decode("ascii", "ignore").rstrip()
    simple_ext = clean_ext.decode("ascii", "ignore").rstrip()
    requested = simple_name if not simple_ext else f"{simple_name}.{simple_ext}"
    if requested == name.upper() and candidate not in used:
        used.add(candidate)
        return candidate, False

    stem = clean_base[:6] or b"FILE"
    for index in range(1, 100000):
        suffix = f"~{index}".encode("ascii")
        short_base = (stem[: 8 - len(suffix)] + suffix)[:8]
        candidate = short_base.ljust(8, b" ") + clean_ext[:3].ljust(3, b" ")
        if candidate not in used:
            used.add(candidate)
            return candidate, True
    raise SystemExit(f"cannot allocate short name for {name}")

def lfn_entry(sequence, chunk, checksum):
    values = [0xFFFF] * 13
    for index, codepoint in enumerate(chunk):
        values[index] = codepoint
    if len(chunk) < 13:
        values[len(chunk)] = 0

    entry = bytearray(32)
    entry[0] = sequence
    entry[11] = 0x0F
    entry[13] = checksum
    for i in range(5):
        struct.pack_into("<H", entry, 1 + i * 2, values[i])
    for i in range(6):
        struct.pack_into("<H", entry, 14 + i * 2, values[5 + i])
    for i in range(2):
        struct.pack_into("<H", entry, 28 + i * 2, values[11 + i])
    return bytes(entry)

def directory_entry(name11, attr, first_cluster, size):
    entry = bytearray(32)
    entry[0:11] = name11
    entry[11] = attr
    struct.pack_into("<H", entry, 20, (first_cluster >> 16) & 0xFFFF)
    struct.pack_into("<H", entry, 26, first_cluster & 0xFFFF)
    struct.pack_into("<I", entry, 28, size)
    return bytes(entry)

class Directory:
    def __init__(self, first_cluster):
        self.first_cluster = first_cluster
        self.entries = []
        self.used_short_names = set()

    def add(self, name, attr, first_cluster, size):
        short, needs_lfn = make_short_name(name, self.used_short_names)
        if needs_lfn or name.upper() != short_to_display_name(short):
            checksum = short_name_checksum(short)
            codes = [ord(ch) for ch in name]
            chunks = [codes[i : i + 13] for i in range(0, len(codes), 13)] or [[]]
            for index in range(len(chunks), 0, -1):
                seq = index
                if index == len(chunks):
                    seq |= 0x40
                self.entries.append(lfn_entry(seq, chunks[index - 1], checksum))
        self.entries.append(directory_entry(short, attr, first_cluster, size))

def short_to_display_name(name11):
    base = name11[:8].decode("ascii").rstrip()
    ext = name11[8:].decode("ascii").rstrip()
    return base if not ext else f"{base}.{ext}"

disk_guid = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-qemu:{os.path.abspath(path)}").bytes_le
disk_signature = zlib.crc32(disk_guid) & 0xFFFFFFFF
esp_type = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").bytes_le
ms_basic_type = uuid.UUID("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7").bytes_le

def fat32_geometry(part_sectors):
    reserved_sectors = 32
    num_fats = 2
    sectors_per_cluster = 1
    fat_size = 1
    while True:
        data_sectors = part_sectors - reserved_sectors - num_fats * fat_size
        if data_sectors <= 0:
            raise SystemExit("partition is too small for FAT32")
        cluster_count = data_sectors // sectors_per_cluster
        required = math.ceil((cluster_count + 2) * 4 / sector_size)
        if required <= fat_size:
            break
        fat_size = required

    if cluster_count < 65525:
        raise SystemExit(
            f"partition is too small for FAT32 with {sector_size}B sectors"
        )

    return reserved_sectors, num_fats, sectors_per_cluster, fat_size, cluster_count

def log2_power_of_two(value):
    if value <= 0 or value & (value - 1):
        raise SystemExit(f"{value} is not a power of two")
    return value.bit_length() - 1

def exfat_geometry(part_sectors):
    boot_region_sectors = 24
    sectors_per_cluster = max(1, 4096 // sector_size)
    fat_offset = boot_region_sectors
    fat_length = 1
    while True:
        cluster_heap_offset = fat_offset + fat_length
        if part_sectors <= cluster_heap_offset:
            raise SystemExit("partition is too small for exFAT")
        cluster_count = (part_sectors - cluster_heap_offset) // sectors_per_cluster
        required = math.ceil((cluster_count + 2) * 4 / sector_size)
        if required <= fat_length:
            break
        fat_length = required
    if cluster_count < 16:
        raise SystemExit("partition is too small for exFAT")
    return fat_offset, fat_length, cluster_heap_offset, cluster_count, sectors_per_cluster

def ntfs_geometry(part_sectors):
    sectors_per_cluster = 1
    cluster_count = part_sectors // sectors_per_cluster
    file_record_size = max(1024, sector_size)
    index_record_size = file_record_size
    if cluster_count < 128:
        raise SystemExit("partition is too small for NTFS")
    return sectors_per_cluster, cluster_count, file_record_size, index_record_size

def make_partition(name, label, fs_type, type_guid, start_lba, end_lba, include_efi, include_images):
    if start_lba < first_usable_lba or end_lba > last_usable_lba or end_lba < start_lba:
        raise SystemExit(f"invalid partition range for {name}")
    part_sectors = end_lba - start_lba + 1
    if fs_type == "fat32":
        fat32_geometry(part_sectors)
    elif fs_type == "exfat":
        exfat_geometry(part_sectors)
    elif fs_type in ("ext2", "ext3", "ext4"):
        if sector_size != 4096:
            raise SystemExit("test ext-family volumes require 4096 byte sectors")
        if part_sectors <= 256:
            raise SystemExit(f"partition is too small for {fs_type}")
    elif fs_type == "ntfs":
        ntfs_geometry(part_sectors)
    elif fs_type == "udf":
        if part_sectors <= 512:
            raise SystemExit("partition is too small for UDF")
    elif fs_type == "xfs":
        if sector_size != 4096:
            raise SystemExit("test XFS volumes require 4096 byte sectors")
        if part_sectors <= 256:
            raise SystemExit("partition is too small for XFS")
    else:
        raise SystemExit(f"unsupported test partition filesystem: {fs_type}")
    return {
        "name": name,
        "label": label,
        "fs_type": fs_type,
        "type_guid": type_guid,
        "guid": uuid.uuid5(
            uuid.NAMESPACE_URL,
            f"nextboot-qemu-part:{os.path.abspath(path)}:{name}",
        ).bytes_le,
        "start_lba": start_lba,
        "end_lba": end_lba,
        "include_efi": include_efi,
        "include_images": include_images,
    }

single_start_lba = align_up(first_usable_lba, alignment_lba)
partitions = []
if layout == "single":
    partitions.append(
        make_partition(
            "NEXBOOT",
            "NEXBOOT",
            "fat32",
            esp_type,
            single_start_lba,
            last_usable_lba,
            True,
            True,
        )
    )
else:
    esp_size_mib = 64 if sector_size == 512 else 260
    esp_start_lba = single_start_lba
    esp_end_lba = esp_start_lba + mib_to_sectors(esp_size_mib) - 1
    data_start_lba = align_up(esp_end_lba + 1, alignment_lba)
    data_end_lba = last_usable_lba
    partitions.append(
        make_partition(
            "NEXBOOT_EFI",
            "NEXBOOT",
            "fat32",
            esp_type,
            esp_start_lba,
            esp_end_lba,
            True,
            False,
        )
    )
    partitions.append(
        make_partition(
            "NEXBOOT_DATA",
            "NEXTDATA",
            data_fs,
            ms_basic_type,
            data_start_lba,
            data_end_lba,
            False,
            True,
        )
    )

def crc32c(data, crc=0):
    crc ^= 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            mask = -(crc & 1) & 0xFFFFFFFF
            crc = ((crc >> 1) ^ (0x82F63B78 & mask)) & 0xFFFFFFFF
    return crc ^ 0xFFFFFFFF

def make_vlnk_file(disksig, part_offset_bytes, target_path):
    data = bytearray(32768)
    data[0:16] = bytes.fromhex("20207777772e76656e746f792e6e6574")
    struct.pack_into("<I", data, 20, disksig)
    struct.pack_into("<Q", data, 24, part_offset_bytes)
    encoded = target_path.encode("utf-8")
    if len(encoded) >= 384:
        raise SystemExit(f"VLNK target path is too long: {target_path}")
    data[32:32 + len(encoded)] = encoded
    crc = crc32c(data[:512])
    struct.pack_into("<I", data, 16, crc)
    return bytes(data)

if smoke_vlnk_iso:
    if not image_files:
        raise SystemExit("VLNK smoke requires a generated image file")
    target_part = next((part for part in partitions if part["include_images"]), None)
    if target_part is None:
        raise SystemExit("VLNK smoke requires an image/data partition")
    with open(image_files[0], "rb") as src:
        extra_files.append(("/ventoy/vlnk-target.iso", src.read()))
    vlnk_data = make_vlnk_file(
        disk_signature,
        target_part["start_lba"] * sector_size,
        "/ventoy/vlnk-target.iso",
    )
    if smoke_vlnk_file:
        with open(smoke_vlnk_file, "wb") as dst:
            dst.write(vlnk_data)
    extra_files.append(
        (
            "/ISO/nextboot-smoke-vlnk.vlnk.iso",
            vlnk_data,
        )
    )

def fat_label(label):
    return label.upper().encode("ascii", "ignore")[:11].ljust(11, b" ")

def partition_name_bytes(name):
    encoded = name.encode("utf-16le")[:72]
    return encoded + bytes(72 - len(encoded))





volume_deps = {
    "sector_size": sector_size,
    "fat32_geometry": fat32_geometry,
    "exfat_geometry": exfat_geometry,
    "ntfs_geometry": ntfs_geometry,
    "log2_power_of_two": log2_power_of_two,
    "align_up": align_up,
    "efi_file": efi_file,
    "smoke_vlnk_iso": smoke_vlnk_iso,
    "image_files": image_files,
    "extra_files": extra_files,
    "Directory": Directory,
    "split_virtual_path": split_virtual_path,
    "fat_label": fat_label,
}

with open(path, "wb") as f:
    f.truncate(total_sectors * sector_size)

    mbr = bytearray(sector_size)
    mbr[0x1B8:0x1BC] = struct.pack("<I", disk_signature)
    mbr[0x1BE] = 0x00
    mbr[0x1BE + 4] = 0xEE
    mbr[0x1BE + 8:0x1BE + 12] = struct.pack("<I", 1)
    protective_size = min(total_sectors - 1, 0xFFFFFFFF)
    mbr[0x1BE + 12:0x1BE + 16] = struct.pack("<I", protective_size)
    mbr[510:512] = b"\x55\xaa"
    f.seek(0)
    f.write(mbr)

    entries = bytearray(entry_count * entry_size)
    for index, part in enumerate(partitions):
        entry = bytearray(entry_size)
        entry[0:16] = part["type_guid"]
        entry[16:32] = part["guid"]
        entry[32:40] = struct.pack("<Q", part["start_lba"])
        entry[40:48] = struct.pack("<Q", part["end_lba"])
        entry[56:128] = partition_name_bytes(part["name"])
        start = index * entry_size
        entries[start:start + entry_size] = entry
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    def make_header(current_lba, backup_lba, entries_lba):
        header = bytearray(sector_size)
        header[0:8] = b"EFI PART"
        header[8:12] = struct.pack("<I", 0x00010000)
        header[12:16] = struct.pack("<I", 92)
        header[24:32] = struct.pack("<Q", current_lba)
        header[32:40] = struct.pack("<Q", backup_lba)
        header[40:48] = struct.pack("<Q", first_usable_lba)
        header[48:56] = struct.pack("<Q", last_usable_lba)
        header[56:72] = disk_guid
        header[72:80] = struct.pack("<Q", entries_lba)
        header[80:84] = struct.pack("<I", entry_count)
        header[84:88] = struct.pack("<I", entry_size)
        header[88:92] = struct.pack("<I", entries_crc)
        crc = zlib.crc32(header[:92]) & 0xFFFFFFFF
        header[16:20] = struct.pack("<I", crc)
        return header

    f.seek(primary_entries_lba * sector_size)
    f.write(entries)
    f.seek(backup_entries_lba * sector_size)
    f.write(entries)
    f.seek(sector_size)
    f.write(make_header(1, last_lba, primary_entries_lba))
    f.seek(last_lba * sector_size)
    f.write(make_header(last_lba, 1, backup_entries_lba))

    for part in partitions:
        if part["fs_type"] == "exfat":
            write_exfat_volume(f, part, volume_deps)
        elif part["fs_type"] in ("ext2", "ext3", "ext4"):
            write_ext4_volume(f, part, volume_deps)
        elif part["fs_type"] == "ntfs":
            write_ntfs_volume(f, part, volume_deps)
        elif part["fs_type"] == "udf":
            write_udf_volume(f, part, volume_deps)
        elif part["fs_type"] == "xfs":
            write_xfs_volume(f, part, volume_deps)
        else:
            write_fat32_volume(f, part, volume_deps)
