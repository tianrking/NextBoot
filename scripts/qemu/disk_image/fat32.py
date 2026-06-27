import math
import os
import struct
import time

def write_fat32_volume(f, part, deps):
    sector_size = deps["sector_size"]
    fat32_geometry = deps["fat32_geometry"]
    efi_file = deps["efi_file"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    Directory = deps["Directory"]
    split_virtual_path = deps["split_virtual_path"]
    fat_label = deps["fat_label"]
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    reserved_sectors, num_fats, sectors_per_cluster, fat_size, cluster_count = (
        fat32_geometry(part_sectors)
    )
    media = 0xF8
    cluster_size = sectors_per_cluster * sector_size
    fat_offset = partition_offset + reserved_sectors * sector_size
    data_offset = partition_offset + (reserved_sectors + num_fats * fat_size) * sector_size
    fat = bytearray(fat_size * sector_size)
    next_cluster = 2

    def set_fat(cluster, value):
        struct.pack_into("<I", fat, cluster * 4, value & 0x0FFFFFFF)

    def cluster_offset(cluster):
        return data_offset + (cluster - 2) * cluster_size

    def allocate_cluster():
        nonlocal next_cluster
        if next_cluster >= cluster_count + 2:
            raise SystemExit(f"{part['name']} is too small for requested files")
        cluster = next_cluster
        next_cluster += 1
        set_fat(cluster, 0x0FFFFFFF)
        return cluster

    def allocate_chain(count):
        if count == 0:
            return []
        chain = [allocate_cluster() for _ in range(count)]
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], 0x0FFFFFFF)
        return chain

    def write_cluster(cluster, data):
        f.seek(cluster_offset(cluster))
        if len(data) > cluster_size:
            raise SystemExit("internal error: cluster write too large")
        f.write(data)
        if len(data) < cluster_size:
            f.write(bytes(cluster_size - len(data)))

    def copy_file(source, target_dir, target_name):
        size = os.path.getsize(source)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        with open(source, "rb") as src:
            for cluster in chain:
                write_cluster(cluster, src.read(cluster_size))
        first = chain[0] if chain else 0
        target_dir.add(target_name, 0x20, first, size)

    def copy_bytes(data, target_dir, target_name):
        size = len(data)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        for index, cluster in enumerate(chain):
            start = index * cluster_size
            write_cluster(cluster, data[start : start + cluster_size])
        first = chain[0] if chain else 0
        target_dir.add(target_name, 0x20, first, size)

    def flush_directory(directory):
        content = b"".join(directory.entries)
        needed = max(1, math.ceil((len(content) + 32) / cluster_size))
        chain = [directory.first_cluster]
        current = directory.first_cluster
        while True:
            value = struct.unpack_from("<I", fat, current * 4)[0] & 0x0FFFFFFF
            if value >= 0x0FFFFFF8:
                break
            chain.append(value)
            current = value
        while len(chain) < needed:
            new_cluster = allocate_cluster()
            set_fat(chain[-1], new_cluster)
            set_fat(new_cluster, 0x0FFFFFFF)
            chain.append(new_cluster)
        content += b"\x00" * (len(chain) * cluster_size - len(content))
        for index, cluster in enumerate(chain):
            write_cluster(cluster, content[index * cluster_size : (index + 1) * cluster_size])

    set_fat(0, 0x0FFFFF00 | media)
    set_fat(1, 0x0FFFFFFF)

    root = Directory(allocate_cluster())
    directories = [root]
    dirs_by_path = {"/": root}

    def ensure_directory(path):
        current = root
        current_path = "/"
        components = [] if path in ("", "/") else split_virtual_path(path)
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
        copy_file(efi_file, boot, "BOOTX64.EFI")

    if part["include_images"]:
        if not smoke_vlnk_iso:
            iso = ensure_directory("/ISO")
            for image in image_files:
                copy_file(image, iso, os.path.basename(image))
        for virtual_path, data in extra_files:
            parts = split_virtual_path(virtual_path)
            target_dir = ensure_directory("/" + "/".join(parts[:-1]))
            copy_bytes(data, target_dir, parts[-1])

    for directory in directories:
        flush_directory(directory)

    if part_sectors > 0xFFFFFFFF:
        raise SystemExit("FAT32 test partition is too large")

    volume_id = int(time.time()) & 0xFFFFFFFF
    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x58\x90"
    boot_sector[3:11] = b"MSWIN4.1"
    struct.pack_into("<H", boot_sector, 11, sector_size)
    boot_sector[13] = sectors_per_cluster
    struct.pack_into("<H", boot_sector, 14, reserved_sectors)
    boot_sector[16] = num_fats
    boot_sector[21] = media
    struct.pack_into("<H", boot_sector, 24, 63)
    struct.pack_into("<H", boot_sector, 26, 255)
    struct.pack_into("<I", boot_sector, 28, part_start_lba)
    struct.pack_into("<I", boot_sector, 32, part_sectors)
    struct.pack_into("<I", boot_sector, 36, fat_size)
    struct.pack_into("<I", boot_sector, 44, root.first_cluster)
    struct.pack_into("<H", boot_sector, 48, 1)
    struct.pack_into("<H", boot_sector, 50, 6)
    boot_sector[64] = 0x80
    boot_sector[66] = 0x29
    struct.pack_into("<I", boot_sector, 67, volume_id)
    boot_sector[71:82] = fat_label(part["label"])
    boot_sector[82:90] = b"FAT32   "
    boot_sector[510:512] = b"\x55\xaa"

    fsinfo = bytearray(sector_size)
    struct.pack_into("<I", fsinfo, 0, 0x41615252)
    struct.pack_into("<I", fsinfo, 484, 0x61417272)
    struct.pack_into("<I", fsinfo, 488, max(0, cluster_count - next_cluster))
    struct.pack_into("<I", fsinfo, 492, next_cluster)
    struct.pack_into("<I", fsinfo, 508, 0xAA550000)

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + sector_size)
    f.write(fsinfo)
    f.seek(partition_offset + 6 * sector_size)
    f.write(boot_sector)
    f.seek(partition_offset + 7 * sector_size)
    f.write(fsinfo)
    for index in range(num_fats):
        f.seek(fat_offset + index * fat_size * sector_size)
        f.write(fat)
