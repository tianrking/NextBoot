import math
import os
import struct
import time

EXFAT_CLUSTER_FREE = 0x00000000
EXFAT_CLUSTER_EOC = 0xFFFFFFFF


def rotate_checksum32(checksum, byte):
    checksum = ((checksum >> 1) | ((checksum & 1) << 31)) & 0xFFFFFFFF
    return (checksum + byte) & 0xFFFFFFFF


def rotate_checksum16(checksum, byte):
    checksum = ((checksum >> 1) | ((checksum & 1) << 15)) & 0xFFFF
    return (checksum + byte) & 0xFFFF


def exfat_boot_checksum(main_boot_region):
    checksum = 0
    for offset, byte in enumerate(main_boot_region):
        if offset in (106, 107, 112):
            continue
        checksum = rotate_checksum32(checksum, byte)
    return checksum


def exfat_table_checksum(data):
    checksum = 0
    for byte in data:
        checksum = rotate_checksum32(checksum, byte)
    return checksum


def exfat_name_hash(name):
    checksum = 0
    for byte in name.upper().encode("utf-16le"):
        checksum = rotate_checksum16(checksum, byte)
    return checksum


def exfat_entry_set_checksum(entries):
    checksum = 0
    for offset, byte in enumerate(entries):
        if offset in (2, 3):
            continue
        checksum = rotate_checksum16(checksum, byte)
    return checksum


def exfat_upcase_table():
    table = bytearray()
    for codepoint in range(0x10000):
        char = chr(codepoint)
        upper = char.upper()
        if len(upper) != 1:
            mapped = codepoint
        else:
            mapped = ord(upper)
            if mapped > 0xFFFF:
                mapped = codepoint
        table.extend(struct.pack("<H", mapped))
    return bytes(table)


def exfat_allocation_bitmap(cluster_count, allocated_clusters):
    data = bytearray(math.ceil(cluster_count / 8))
    for cluster in allocated_clusters:
        if 2 <= cluster < cluster_count + 2:
            index = cluster - 2
            data[index // 8] |= 1 << (index % 8)
    return bytes(data)


def exfat_volume_label_entry(label):
    entry = bytearray(32)
    encoded = label.encode("utf-16le")
    code_units = len(encoded) // 2
    entry[0] = 0x83
    entry[1] = min(code_units, 11)
    entry[2 : 2 + min(len(encoded), 22)] = encoded[:22]
    return bytes(entry)


def exfat_allocation_bitmap_entry(first_cluster, size):
    entry = bytearray(32)
    entry[0] = 0x81
    struct.pack_into("<I", entry, 20, first_cluster)
    struct.pack_into("<Q", entry, 24, size)
    return bytes(entry)


def exfat_upcase_table_entry(first_cluster, size, checksum):
    entry = bytearray(32)
    entry[0] = 0x82
    struct.pack_into("<I", entry, 4, checksum)
    struct.pack_into("<I", entry, 20, first_cluster)
    struct.pack_into("<Q", entry, 24, size)
    return bytes(entry)


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
    struct.pack_into("<H", stream_entry, 4, exfat_name_hash(name))
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

    entry_set = bytearray(b"".join(entries))
    struct.pack_into("<H", entry_set, 2, exfat_entry_set_checksum(entry_set))
    return bytes(entry_set)


def exfat_boot_regions(sector_size, part_start_lba, part_sectors, fat_offset, fat_length,
                       cluster_heap_offset, cluster_count, root_cluster,
                       sectors_per_cluster, volume_id, percent_in_use):
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
    boot_sector[108] = int(math.log2(sector_size))
    boot_sector[109] = int(math.log2(sectors_per_cluster))
    boot_sector[110] = 1
    boot_sector[111] = 0x80
    boot_sector[112] = percent_in_use
    boot_sector[510:512] = b"\x55\xaa"

    main_region = bytearray(12 * sector_size)
    main_region[0:sector_size] = boot_sector
    for sector in range(1, 9):
        start = sector * sector_size
        main_region[start + sector_size - 4 : start + sector_size] = b"\x00\x00\x55\xaa"
    checksum = exfat_boot_checksum(main_region[: 11 * sector_size])
    checksum_sector = struct.pack("<I", checksum) * (sector_size // 4)
    main_region[11 * sector_size : 12 * sector_size] = checksum_sector
    return bytes(main_region + main_region)

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
    allocated_clusters = set()

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
        allocated_clusters.update(chain)
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], EXFAT_CLUSTER_EOC)
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
    set_fat(1, EXFAT_CLUSTER_EOC)

    bitmap_cluster_capacity = max(cluster_count, fat_length * sector_size // 4 - 2)
    bitmap_size = math.ceil(bitmap_cluster_capacity / 8)
    bitmap_chain = allocate_chain(math.ceil(bitmap_size / cluster_size))
    bitmap_first_cluster = bitmap_chain[0]
    upcase_table = exfat_upcase_table()
    upcase_chain = allocate_chain(math.ceil(len(upcase_table) / cluster_size))
    upcase_first_cluster = upcase_chain[0]
    write_chain(upcase_chain, upcase_table)

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

    root_entries = [
        exfat_volume_label_entry("NEXTDATA"),
        exfat_allocation_bitmap_entry(bitmap_first_cluster, bitmap_size),
        exfat_upcase_table_entry(
            upcase_first_cluster,
            len(upcase_table),
            exfat_table_checksum(upcase_table),
        ),
    ]

    def write_root_directory(directory):
        entry_sets = list(root_entries)
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

    root_cluster, _root_size = write_root_directory(root)

    bitmap = exfat_allocation_bitmap(bitmap_cluster_capacity, allocated_clusters)
    write_chain(bitmap_chain, bitmap)

    volume_id = int(time.time()) & 0xFFFFFFFF
    percent_in_use = min(100, int(len(allocated_clusters) * 100 / max(1, cluster_count)))

    f.seek(partition_offset)
    f.write(
        exfat_boot_regions(
            sector_size,
            part_start_lba,
            part_sectors,
            fat_offset,
            fat_length,
            cluster_heap_offset,
            cluster_count,
            root_cluster,
            sectors_per_cluster,
            volume_id,
            percent_in_use,
        )
    )
    f.seek(partition_offset + fat_offset * sector_size)
    f.write(fat)
