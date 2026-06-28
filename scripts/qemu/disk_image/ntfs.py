import math
import os
import struct
import time

def write_ntfs_volume(f, part, deps):
    sector_size = deps["sector_size"]
    ntfs_geometry = deps["ntfs_geometry"]
    log2_power_of_two = deps["log2_power_of_two"]
    align_up = deps["align_up"]
    efi_entries = deps["efi_entries"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    sectors_per_cluster, cluster_count, file_record_size, index_record_size = ntfs_geometry(
        part_sectors
    )
    cluster_size = sectors_per_cluster * sector_size
    mft_lcn = 4

    attr_type_data = 0x80
    attr_type_index_root = 0x90
    attr_type_file_name = 0x30
    attr_type_end = 0xFFFFFFFF
    file_attribute_archive = 0x00000020
    file_attribute_directory = 0x10000000
    index_entry_last = 0x0002

    class NtfsNode:
        def __init__(self, name, is_dir, source=None, data=None):
            self.name = name
            self.is_dir = is_dir
            self.source = source
            self.data = data
            self.children = []
            self.children_by_name = {}
            self.record = 0
            self.size = 0
            self.lcn = 0
            self.clusters = 0

    root = NtfsNode("", True)
    root.record = 5

    def ensure_ntfs_directory(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = NtfsNode(component, True)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"NTFS path component is not a directory: {component}")
        return current

    def add_ntfs_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_ntfs_directory("/" + "/".join(parts[:-1]))
        node = NtfsNode(parts[-1], False, source=source, data=data)
        node.size = os.path.getsize(source) if source is not None else len(data or b"")
        directory.children.append(node)
        directory.children_by_name[node.name.lower()] = node

    if part["include_efi"]:
        for efi_boot_name, efi_file in efi_entries:
            add_ntfs_file(f"/EFI/BOOT/{efi_boot_name}", source=efi_file)

    if part["include_images"]:
        if not smoke_vlnk_iso:
            ensure_ntfs_directory("/ISO")
            for image in image_files:
                add_ntfs_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_ntfs_file(virtual_path, data=data)

    next_record = 6

    def assign_records(directory):
        nonlocal next_record
        for child in directory.children:
            child.record = next_record
            next_record += 1
            if child.is_dir:
                assign_records(child)

    assign_records(root)
    mft_record_count = next_record
    mft_bytes = mft_record_count * file_record_size
    mft_clusters = math.ceil(mft_bytes / cluster_size)
    next_cluster = max(64, mft_lcn + mft_clusters + 8)

    def allocate_clusters(count):
        nonlocal next_cluster
        if count == 0:
            return 0
        if next_cluster + count > cluster_count:
            raise SystemExit(f"{part['name']} is too small for requested NTFS files")
        start = next_cluster
        next_cluster += count
        return start

    def write_node_payloads(node):
        if node.is_dir:
            for child in node.children:
                write_node_payloads(child)
            return

        node.clusters = math.ceil(node.size / cluster_size) if node.size else 0
        node.lcn = allocate_clusters(node.clusters)
        if node.clusters == 0:
            return

        f.seek(partition_offset + node.lcn * cluster_size)
        if node.source is not None:
            with open(node.source, "rb") as src:
                remaining = node.size
                while remaining > 0:
                    chunk = src.read(min(cluster_size, remaining))
                    if not chunk:
                        raise SystemExit(f"short read while copying {node.source}")
                    f.write(chunk)
                    remaining -= len(chunk)
                padding = node.clusters * cluster_size - node.size
                if padding:
                    f.write(bytes(padding))
        else:
            content = node.data or b""
            f.write(content)
            padding = node.clusters * cluster_size - len(content)
            if padding:
                f.write(bytes(padding))

    write_node_payloads(root)

    def ntfs_record_size_code(byte_size):
        if byte_size >= cluster_size and byte_size % cluster_size == 0:
            value = byte_size // cluster_size
            if 1 <= value <= 127:
                return value
        if byte_size > 0 and byte_size & (byte_size - 1) == 0:
            shift = log2_power_of_two(byte_size)
            if shift < 128:
                return (-shift) & 0xFF
        raise SystemExit(f"unsupported NTFS record size: {byte_size}")

    def uint_le_bytes(value):
        if value < 0:
            raise SystemExit("negative unsigned NTFS run value")
        size = max(1, (value.bit_length() + 7) // 8)
        return value.to_bytes(size, "little")

    def int_le_bytes(value):
        for size in range(1, 9):
            try:
                encoded = value.to_bytes(size, "little", signed=True)
            except OverflowError:
                continue
            if int.from_bytes(encoded, "little", signed=True) == value:
                return encoded
        raise SystemExit("NTFS run delta is too large")

    def ntfs_data_runs(runs):
        out = bytearray()
        previous_lcn = 0
        for lcn, length in runs:
            length_bytes = uint_le_bytes(length)
            delta_bytes = int_le_bytes(lcn - previous_lcn)
            previous_lcn = lcn
            if len(length_bytes) > 8 or len(delta_bytes) > 8:
                raise SystemExit("NTFS run field is too large")
            out.append((len(delta_bytes) << 4) | len(length_bytes))
            out.extend(length_bytes)
            out.extend(delta_bytes)
        out.append(0)
        return bytes(out)

    def ntfs_data_attr_nonresident(real_size, runs):
        runlist = ntfs_data_runs(runs)
        total_clusters = sum(length for _lcn, length in runs)
        allocated_size = total_clusters * cluster_size
        runlist_offset = 0x40
        attr_len = align_up(runlist_offset + len(runlist), 8)
        attr = bytearray(attr_len)
        struct.pack_into("<I", attr, 0, attr_type_data)
        struct.pack_into("<I", attr, 4, attr_len)
        attr[8] = 1
        highest_vcn = total_clusters - 1 if total_clusters else 0
        struct.pack_into("<Q", attr, 0x18, highest_vcn)
        struct.pack_into("<H", attr, 0x20, runlist_offset)
        struct.pack_into("<Q", attr, 0x28, allocated_size)
        struct.pack_into("<Q", attr, 0x30, real_size)
        struct.pack_into("<Q", attr, 0x38, real_size)
        attr[runlist_offset : runlist_offset + len(runlist)] = runlist
        return bytes(attr)

    def ntfs_resident_attr(attr_type, value):
        value_offset = 0x18
        attr_len = align_up(value_offset + len(value), 8)
        attr = bytearray(attr_len)
        struct.pack_into("<I", attr, 0, attr_type)
        struct.pack_into("<I", attr, 4, attr_len)
        struct.pack_into("<I", attr, 0x10, len(value))
        struct.pack_into("<H", attr, 0x14, value_offset)
        attr[value_offset : value_offset + len(value)] = value
        return bytes(attr)

    def ntfs_index_entry(node):
        attrs = file_attribute_directory if node.is_dir else file_attribute_archive
        allocated = 0 if node.is_dir else align_up(node.size, cluster_size)
        name_units = list(node.name.encode("utf-16le"))
        name_len = len(name_units) // 2
        file_name = bytearray(66 + len(name_units))
        struct.pack_into("<Q", file_name, 40, allocated)
        struct.pack_into("<Q", file_name, 48, node.size)
        struct.pack_into("<I", file_name, 56, attrs)
        file_name[64] = name_len
        file_name[65] = 1
        file_name[66 : 66 + len(name_units)] = bytes(name_units)

        entry_len = align_up(16 + len(file_name), 8)
        entry = bytearray(entry_len)
        entry[0:6] = node.record.to_bytes(8, "little")[:6]
        struct.pack_into("<H", entry, 8, entry_len)
        struct.pack_into("<H", entry, 10, len(file_name))
        entry[16 : 16 + len(file_name)] = file_name
        return bytes(entry)

    def ntfs_index_root_attr(children):
        entries = [ntfs_index_entry(child) for child in children]
        value = bytearray(32)
        struct.pack_into("<I", value, 0, attr_type_file_name)
        struct.pack_into("<I", value, 8, index_record_size)
        value[12] = 1
        struct.pack_into("<I", value, 16, 16)
        entries_len = sum(len(entry) for entry in entries)
        for entry in entries:
            value.extend(entry)
        last = bytearray(16)
        struct.pack_into("<H", last, 8, 16)
        struct.pack_into("<H", last, 12, index_entry_last)
        entries_len += len(last)
        value.extend(last)
        total = 16 + entries_len
        struct.pack_into("<I", value, 20, total)
        struct.pack_into("<I", value, 24, total)
        return ntfs_resident_attr(attr_type_index_root, value)

    def ntfs_apply_fixup(record):
        sector_count = len(record) // sector_size
        if sector_count == 0 or len(record) % sector_size != 0:
            raise SystemExit("NTFS record size is not sector aligned")
        usa_offset = 0x30
        usa_count = sector_count + 1
        sequence = 0xA55A
        struct.pack_into("<H", record, 4, usa_offset)
        struct.pack_into("<H", record, 6, usa_count)
        struct.pack_into("<H", record, usa_offset, sequence)
        for sector in range(sector_count):
            tail = (sector + 1) * sector_size - 2
            original = record[tail : tail + 2]
            record[usa_offset + 2 * (sector + 1) : usa_offset + 2 * (sector + 2)] = original
            struct.pack_into("<H", record, tail, sequence)

    def ntfs_mft_record(is_dir, attrs):
        record = bytearray(file_record_size)
        record[0:4] = b"FILE"
        attrs_offset = 0x38
        struct.pack_into("<H", record, 0x10, 1)
        struct.pack_into("<H", record, 0x14, attrs_offset)
        struct.pack_into("<H", record, 0x16, 0x0003 if is_dir else 0x0001)
        cursor = attrs_offset
        for attr in attrs:
            end = cursor + len(attr)
            if end + 4 > len(record):
                raise SystemExit("NTFS MFT record is too small for generated attributes")
            record[cursor:end] = attr
            cursor = end
        struct.pack_into("<I", record, cursor, attr_type_end)
        cursor += 4
        struct.pack_into("<I", record, 0x18, cursor)
        struct.pack_into("<I", record, 0x1C, file_record_size)
        ntfs_apply_fixup(record)
        return record

    def write_record(record_number, is_dir, attrs):
        offset = partition_offset + mft_lcn * cluster_size + record_number * file_record_size
        f.seek(offset)
        f.write(ntfs_mft_record(is_dir, attrs))

    mft_attr = ntfs_data_attr_nonresident(
        mft_clusters * cluster_size, [(mft_lcn, mft_clusters)]
    )
    write_record(0, False, [mft_attr])

    def write_node_records(node):
        if node.is_dir:
            write_record(node.record, True, [ntfs_index_root_attr(node.children)])
            for child in node.children:
                write_node_records(child)
        else:
            runs = [(node.lcn, node.clusters)] if node.clusters else []
            write_record(node.record, False, [ntfs_data_attr_nonresident(node.size, runs)])

    write_node_records(root)

    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x52\x90"
    boot_sector[3:11] = b"NTFS    "
    struct.pack_into("<H", boot_sector, 0x0B, sector_size)
    boot_sector[0x0D] = sectors_per_cluster
    struct.pack_into("<Q", boot_sector, 0x28, part_sectors)
    struct.pack_into("<Q", boot_sector, 0x30, mft_lcn)
    struct.pack_into("<Q", boot_sector, 0x38, max(8, cluster_count // 2))
    boot_sector[0x40] = ntfs_record_size_code(file_record_size)
    boot_sector[0x44] = ntfs_record_size_code(index_record_size)
    struct.pack_into("<Q", boot_sector, 0x48, int(time.time()))
    boot_sector[510:512] = b"\x55\xaa"

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + (part_sectors - 1) * sector_size)
    f.write(boot_sector)
