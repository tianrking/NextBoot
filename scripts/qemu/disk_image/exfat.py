import math
import os
import struct
import time

def exfat_entry_set(name, attr, first_cluster, size, contiguous):
    encoded_name = name.encode("utf-16le")
    code_units = [encoded_name[i] | (encoded_name[i + 1] << 8) for i in range(0, len(encoded_name), 2)]
    name_entries = max(1, math.ceil(len(code_units) / 15))
    secondary_count = 1 + name_entries

    file_entry = bytearray(32)
    file_entry[0] = 0x85
    file_entry[1] = secondary_count
    struct.pack_into("<H", file_entry, 4, attr)

    stream_entry = bytearray(32)
    stream_entry[0] = 0xC0
    stream_entry[1] = 0x02 if contiguous else 0
    stream_entry[3] = len(code_units)
    struct.pack_into("<Q", stream_entry, 8, size)
    struct.pack_into("<I", stream_entry, 20, first_cluster)
    struct.pack_into("<Q", stream_entry, 24, size)

    entries = [bytes(file_entry), bytes(stream_entry)]
    for index in range(name_entries):
        name_entry = bytearray(32)
        name_entry[0] = 0xC1
        chunk = code_units[index * 15 : (index + 1) * 15]
        for char_index, value in enumerate(chunk):
            struct.pack_into("<H", name_entry, 2 + char_index * 2, value)
        entries.append(bytes(name_entry))

    return b"".join(entries)

def write_exfat_volume(f, part, deps):
    sector_size = deps["sector_size"]
    exfat_geometry = deps["exfat_geometry"]
    log2_power_of_two = deps["log2_power_of_two"]
    efi_entries = deps["efi_entries"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    fat_offset, fat_length, cluster_heap_offset, cluster_count, sectors_per_cluster = (
        exfat_geometry(part_sectors)
    )
    cluster_size = sectors_per_cluster * sector_size
    fat = bytearray(fat_length * sector_size)
    next_cluster = 2

    def set_fat(cluster, value):
        struct.pack_into("<I", fat, cluster * 4, value & 0xFFFFFFFF)

    def cluster_offset(cluster):
        return partition_offset + (
            cluster_heap_offset + (cluster - 2) * sectors_per_cluster
        ) * sector_size

    def allocate_chain(count):
        nonlocal next_cluster
        if count == 0:
            return []
        if next_cluster + count > cluster_count + 2:
            raise SystemExit(f"{part['name']} is too small for requested files")
        chain = list(range(next_cluster, next_cluster + count))
        next_cluster += count
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], 0xFFFFFFFF)
        return chain

    def write_cluster(cluster, data):
        f.seek(cluster_offset(cluster))
        if len(data) > cluster_size:
            raise SystemExit("internal error: exFAT cluster write too large")
        f.write(data)
        if len(data) < cluster_size:
            f.write(bytes(cluster_size - len(data)))

    def write_chain(chain, data):
        content = data + bytes(len(chain) * cluster_size - len(data))
        for index, cluster in enumerate(chain):
            write_cluster(cluster, content[index * cluster_size : (index + 1) * cluster_size])

    def copy_file(source):
        size = os.path.getsize(source)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        with open(source, "rb") as src:
            for cluster in chain:
                write_cluster(cluster, src.read(cluster_size))
        return (chain[0] if chain else 0, size)

    def copy_bytes(data):
        size = len(data)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        write_chain(chain, data)
        return (chain[0] if chain else 0, size)

    def write_directory(entry_sets):
        content = b"".join(entry_sets)
        clusters_needed = max(1, math.ceil((len(content) + 32) / cluster_size))
        chain = allocate_chain(clusters_needed)
        write_chain(chain, content)
        return chain[0], len(chain) * cluster_size

    set_fat(0, 0xFFFFFFF8)
    set_fat(1, 0xFFFFFFFF)

    class TreeDirectory:
        def __init__(self):
            self.directories = {}
            self.files = []

    root = TreeDirectory()

    def ensure_tree_directory(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            current = current.directories.setdefault(component, TreeDirectory())
        return current

    def add_tree_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_tree_directory("/" + "/".join(parts[:-1]))
        directory.files.append((parts[-1], source, data))

    if part["include_efi"]:
        for efi_boot_name, efi_file in efi_entries:
            add_tree_file(f"/EFI/BOOT/{efi_boot_name}", source=efi_file)

    if part["include_images"]:
        if not smoke_vlnk_iso:
            ensure_tree_directory("/ISO")
            for image in image_files:
                add_tree_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_tree_file(virtual_path, data=data)

    def write_tree_directory(directory):
        entry_sets = []
        for name, child in directory.directories.items():
            child_cluster, child_size = write_tree_directory(child)
            entry_sets.append(exfat_entry_set(name, 0x0010, child_cluster, child_size, False))
        for name, source, data in directory.files:
            if source is not None:
                file_cluster, file_size = copy_file(source)
            else:
                file_cluster, file_size = copy_bytes(data or b"")
            entry_sets.append(exfat_entry_set(name, 0x0020, file_cluster, file_size, True))
        return write_directory(entry_sets)

    root_cluster, _root_size = write_tree_directory(root)

    bytes_per_sector_shift = log2_power_of_two(sector_size)
    sectors_per_cluster_shift = log2_power_of_two(sectors_per_cluster)
    volume_id = int(time.time()) & 0xFFFFFFFF

    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x76\x90"
    boot_sector[3:11] = b"EXFAT   "
    struct.pack_into("<Q", boot_sector, 64, part_start_lba)
    struct.pack_into("<Q", boot_sector, 72, part_sectors)
    struct.pack_into("<I", boot_sector, 80, fat_offset)
    struct.pack_into("<I", boot_sector, 84, fat_length)
    struct.pack_into("<I", boot_sector, 88, cluster_heap_offset)
    struct.pack_into("<I", boot_sector, 92, cluster_count)
    struct.pack_into("<I", boot_sector, 96, root_cluster)
    struct.pack_into("<I", boot_sector, 100, volume_id)
    struct.pack_into("<H", boot_sector, 104, 0x0100)
    struct.pack_into("<H", boot_sector, 106, 0)
    boot_sector[108] = bytes_per_sector_shift
    boot_sector[109] = sectors_per_cluster_shift
    boot_sector[110] = 1
    boot_sector[111] = 0x80
    boot_sector[112] = min(100, int((next_cluster - 2) * 100 / max(1, cluster_count)))
    boot_sector[510:512] = b"\x55\xaa"

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + 12 * sector_size)
    f.write(boot_sector)
    f.seek(partition_offset + fat_offset * sector_size)
    f.write(fat)
