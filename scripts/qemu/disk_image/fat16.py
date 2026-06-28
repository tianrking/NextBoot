import struct
import time

from .fat_names import fat_label


def write_fat16_volume(f, part, deps):
    sector_size = deps["sector_size"]
    fat16_geometry = deps["fat16_geometry"]
    efi_entries = deps["efi_entries"]
    Directory = deps["Directory"]
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    (
        reserved_sectors,
        num_fats,
        sectors_per_cluster,
        fat_size,
        root_entry_count,
        _cluster_count,
    ) = fat16_geometry(part_sectors)
    root_dir_sectors = (root_entry_count * 32 + sector_size - 1) // sector_size
    fat_offset = partition_offset + reserved_sectors * sector_size
    root_dir_offset = fat_offset + num_fats * fat_size * sector_size
    data_offset = root_dir_offset + root_dir_sectors * sector_size
    cluster_size = sectors_per_cluster * sector_size
    next_cluster = 2

    fat = bytearray(fat_size * sector_size)
    struct.pack_into("<H", fat, 0, 0xFFF8)
    struct.pack_into("<H", fat, 2, 0xFFFF)

    def set_fat(cluster, value):
        struct.pack_into("<H", fat, cluster * 2, value & 0xFFFF)

    def cluster_offset(cluster):
        return data_offset + (cluster - 2) * cluster_size

    def allocate_cluster():
        nonlocal next_cluster
        if next_cluster >= _cluster_count + 2:
            raise SystemExit(f"{part['name']} is too small for requested files")
        cluster = next_cluster
        next_cluster += 1
        set_fat(cluster, 0xFFFF)
        return cluster

    def allocate_chain(count):
        if count == 0:
            return []
        chain = [allocate_cluster() for _ in range(count)]
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], 0xFFFF)
        return chain

    def write_cluster(cluster, data):
        f.seek(cluster_offset(cluster))
        if len(data) > cluster_size:
            raise SystemExit("internal error: FAT16 cluster write too large")
        f.write(data)
        if len(data) < cluster_size:
            f.write(bytes(cluster_size - len(data)))

    def copy_file(source, target_dir, target_name):
        import math
        import os

        size = os.path.getsize(source)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        with open(source, "rb") as src:
            for cluster in chain:
                write_cluster(cluster, src.read(cluster_size))
        first = chain[0] if chain else 0
        target_dir.add(target_name, 0x20, first, size)

    root = Directory(0)
    directories = []
    dirs_by_path = {"/": root}

    def ensure_directory(path):
        current = root
        current_path = "/"
        components = [part for part in path.strip("/").split("/") if part]
        for component in components:
            next_path = current_path.rstrip("/") + "/" + component
            if next_path not in dirs_by_path:
                directory = Directory(allocate_cluster())
                current.add(component, 0x10, directory.first_cluster, 0)
                dirs_by_path[next_path] = directory
                directories.append(directory)
            current = dirs_by_path[next_path]
            current_path = next_path
        return current

    if part["include_efi"]:
        boot = ensure_directory("/EFI/BOOT")
        for efi_boot_name, efi_file in efi_entries:
            copy_file(efi_file, boot, efi_boot_name)

    volume_id = int(time.time()) & 0xFFFFFFFF
    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x3c\x90"
    boot_sector[3:11] = b"MSDOS5.0"
    struct.pack_into("<H", boot_sector, 11, sector_size)
    boot_sector[13] = sectors_per_cluster
    struct.pack_into("<H", boot_sector, 14, reserved_sectors)
    boot_sector[16] = num_fats
    struct.pack_into("<H", boot_sector, 17, root_entry_count)
    if part_sectors <= 0xFFFF:
        struct.pack_into("<H", boot_sector, 19, part_sectors)
    else:
        struct.pack_into("<I", boot_sector, 32, part_sectors)
    boot_sector[21] = 0xF8
    struct.pack_into("<H", boot_sector, 22, fat_size)
    struct.pack_into("<H", boot_sector, 24, 63)
    struct.pack_into("<H", boot_sector, 26, 255)
    struct.pack_into("<I", boot_sector, 28, part_start_lba)
    boot_sector[36] = 0x80
    boot_sector[38] = 0x29
    struct.pack_into("<I", boot_sector, 39, volume_id)
    boot_sector[43:54] = fat_label(part["label"])
    boot_sector[54:62] = b"FAT16   "
    boot_sector[510:512] = b"\x55\xaa"

    f.seek(partition_offset)
    f.write(boot_sector)
    for index in range(num_fats):
        f.seek(fat_offset + index * fat_size * sector_size)
        f.write(fat)
    f.seek(root_dir_offset)
    root_content = b"".join(root.entries)
    if len(root_content) > root_dir_sectors * sector_size:
        raise SystemExit(f"{part['name']} root directory is too small")
    f.write(root_content)
    f.write(bytes(root_dir_sectors * sector_size - len(root_content)))

    for directory in directories:
        content = b"".join(directory.entries)
        if len(content) + 32 > cluster_size:
            raise SystemExit(f"{part['name']} generated directory is too large")
        write_cluster(directory.first_cluster, content)
