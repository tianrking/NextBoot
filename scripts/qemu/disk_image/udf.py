import math
import os
import struct


BLOCK_CONTENT_FREE = 0
BLOCK_CONTENT_DIRECTORY = 1
BLOCK_CONTENT_FILE = 2


def write_u16(data, offset, value):
    struct.pack_into("<H", data, offset, value)


def write_u32(data, offset, value):
    struct.pack_into("<I", data, offset, value)


def write_u64(data, offset, value):
    struct.pack_into("<Q", data, offset, value)


def write_tag(block, ident, location):
    block[0:2] = struct.pack("<H", ident)
    block[2:4] = struct.pack("<H", 2)
    block[12:16] = struct.pack("<I", location)


def write_tag_at(block, offset, ident, location):
    write_u16(block, offset, ident)
    write_u16(block, offset + 2, 2)
    write_u32(block, offset + 12, location)


def write_long_ad(block, offset, length, block_num, part_ref=0):
    write_u32(block, offset, length)
    write_u32(block, offset + 4, block_num)
    write_u16(block, offset + 8, part_ref)


def write_short_ad(block, offset, length, position):
    write_u32(block, offset, length)
    write_u32(block, offset + 4, position)


def udf_name(name):
    encoded = name.encode("utf-8")
    if len(encoded) > 254:
        raise SystemExit(f"UDF name is too long: {name}")
    return bytes([8]) + encoded


def write_fid(block, offset, name, icb_block, is_dir):
    raw = udf_name(name)
    write_tag_at(block, offset, 0x0101, 0)
    write_u16(block, offset + 16, 1)
    block[offset + 18] = 0x02 if is_dir else 0
    block[offset + 19] = len(raw)
    write_long_ad(block, offset + 20, block_size_placeholder(block), icb_block)
    write_u16(block, offset + 36, 0)
    block[offset + 38 : offset + 38 + len(raw)] = raw
    return (offset + 38 + len(raw) + 3) & ~3


def block_size_placeholder(block):
    return len(block)


class UdfNode:
    def __init__(self, name, is_dir, source=None, data=None):
        self.name = name
        self.is_dir = is_dir
        self.source = source
        self.data = data
        self.children = []
        self.children_by_name = {}
        self.entry_block = 0
        self.data_block = 0
        self.size = 0
        self.blocks = 0


def write_udf_volume(f, part, deps):
    sector_size = deps["sector_size"]
    efi_file = deps["efi_file"]
    efi_boot_name = deps["efi_boot_name"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]

    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_base = 100
    if part_sectors <= 512 or partition_base + 16 >= part_sectors:
        raise SystemExit("partition is too small for UDF")

    root = UdfNode("", True)

    def ensure_dir(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = UdfNode(component, True)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"UDF path component is not a directory: {component}")
        return current

    def add_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_dir("/" + "/".join(parts[:-1]))
        node = UdfNode(parts[-1], False, source=source, data=data)
        node.size = os.path.getsize(source) if source is not None else len(data or b"")
        directory.children.append(node)
        directory.children_by_name[node.name.lower()] = node

    if part["include_efi"]:
        add_file(f"/EFI/BOOT/{efi_boot_name}", source=efi_file)

    if part["include_images"]:
        if not smoke_vlnk_iso:
            ensure_dir("/ISO")
            for image in image_files:
                add_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_file(virtual_path, data=data)

    next_block = 1

    def allocate_block():
        nonlocal next_block
        block = next_block
        next_block += 1
        if partition_base + next_block >= part_sectors:
            raise SystemExit(f"{part['name']} is too small for requested UDF files")
        return block

    def assign_blocks(node):
        node.entry_block = allocate_block()
        node.data_block = allocate_block()
        if node.is_dir:
            for child in node.children:
                assign_blocks(child)
        else:
            node.blocks = math.ceil(node.size / sector_size) if node.size else 0
            if node.blocks:
                node.data_block = next_block
                for _ in range(node.blocks):
                    allocate_block()

    fileset_block = allocate_block()
    assign_blocks(root)

    def seek_udf_block(relative_block):
        f.seek((part_start_lba + partition_base + relative_block) * sector_size)

    def write_block(relative_block, data):
        if len(data) > sector_size:
            raise SystemExit("internal error: UDF block payload too large")
        seek_udf_block(relative_block)
        f.write(data)
        if len(data) < sector_size:
            f.write(bytes(sector_size - len(data)))

    def serialize_directory(node):
        data = bytearray(sector_size)
        offset = 0
        for child in node.children:
            offset = write_fid(data, offset, child.name, child.entry_block, child.is_dir)
            if offset > len(data):
                raise SystemExit(f"UDF directory is too large: {node.name or '/'}")
        return bytes(data), offset

    def write_file_entry(node, file_type, data_len):
        block = bytearray(sector_size)
        write_tag(block, 0x0105, partition_base + node.entry_block)
        block[27] = file_type
        write_u16(block, 34, 0)
        write_u64(block, 56, data_len)
        write_u64(block, 64, node.blocks * sector_size if not node.is_dir else sector_size)
        write_u32(block, 172, 8 if data_len else 0)
        if data_len:
            write_short_ad(block, 176, data_len, node.data_block)
        write_block(node.entry_block, block)

    def write_node(node):
        if node.is_dir:
            data, used = serialize_directory(node)
            write_block(node.data_block, data)
            write_file_entry(node, 0x04, used)
            for child in node.children:
                write_node(child)
            return

        write_file_entry(node, 0x05, node.size)
        if node.size == 0:
            return
        seek_udf_block(node.data_block)
        remaining = node.size
        if node.source is not None:
            with open(node.source, "rb") as src:
                while remaining > 0:
                    chunk = src.read(min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    f.write(chunk)
                    remaining -= len(chunk)
        else:
            data = node.data or b""
            f.write(data)
            remaining = 0
        padding = node.blocks * sector_size - node.size
        if padding:
            f.write(bytes(padding))

    anchor = bytearray(sector_size)
    write_tag(anchor, 0x0002, 256)
    write_u32(anchor, 16, 4 * sector_size)
    write_u32(anchor, 20, 32)
    f.seek((part_start_lba + 256) * sector_size)
    f.write(anchor)

    pd = bytearray(sector_size)
    write_tag(pd, 0x0005, 32)
    write_u16(pd, 22, 0)
    write_u32(pd, 188, partition_base)
    write_u32(pd, 192, part_sectors - partition_base)
    f.seek((part_start_lba + 32) * sector_size)
    f.write(pd)

    lvd = bytearray(sector_size)
    write_tag(lvd, 0x0006, 33)
    write_u32(lvd, 212, sector_size)
    write_long_ad(lvd, 248, sector_size, fileset_block)
    write_u32(lvd, 264, 6)
    write_u32(lvd, 268, 1)
    lvd[440] = 1
    lvd[441] = 6
    write_u16(lvd, 442, 0)
    write_u16(lvd, 444, 0)
    f.seek((part_start_lba + 33) * sector_size)
    f.write(lvd)

    td = bytearray(sector_size)
    write_tag(td, 0x0008, 34)
    f.seek((part_start_lba + 34) * sector_size)
    f.write(td)

    fsd = bytearray(sector_size)
    write_tag(fsd, 0x0100, fileset_block)
    write_long_ad(fsd, 400, sector_size, root.entry_block)
    write_block(fileset_block, fsd)

    write_node(root)
