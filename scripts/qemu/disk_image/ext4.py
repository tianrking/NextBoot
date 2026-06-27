import math
import os
import struct
import time


EXT4_EXTENTS_FL = 0x00080000
EXT4_FT_REG_FILE = 1
EXT4_FT_DIR = 2


class Ext4Node:
    def __init__(self, name, is_dir, parent=None, source=None, data=None):
        self.name = name
        self.is_dir = is_dir
        self.parent = parent
        self.source = source
        self.data = data
        self.children = []
        self.children_by_name = {}
        self.inode = 0
        self.size = 0
        self.first_block = 0
        self.blocks = 0


def write_u16(data, offset, value):
    struct.pack_into("<H", data, offset, value)


def write_u32(data, offset, value):
    struct.pack_into("<I", data, offset, value)


def write_ext4_volume(f, part, deps):
    block_size = deps["sector_size"]
    if block_size != 4096:
        raise SystemExit("test ext-family volumes require 4096 byte sectors")
    use_extents = part["fs_type"] == "ext4"

    efi_file = deps["efi_file"]
    efi_boot_name = deps["efi_boot_name"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]

    part_start_lba = part["start_lba"]
    part_blocks = part["end_lba"] - part["start_lba"] + 1
    if part_blocks < 256:
        raise SystemExit("partition is too small for ext4")

    root = Ext4Node("", True)

    def ensure_dir(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = Ext4Node(component, True, parent=current)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"ext4 path component is not a directory: {component}")
        return current

    def add_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_dir("/" + "/".join(parts[:-1]))
        node = Ext4Node(parts[-1], False, parent=directory, source=source, data=data)
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

    nodes = []

    def assign_inodes(node):
        node.inode = 2 + len(nodes)
        nodes.append(node)
        for child in node.children:
            assign_inodes(child)

    assign_inodes(root)

    inode_size = 256
    inode_count = max(64, len(nodes) + 8)
    inode_table_blocks = math.ceil(inode_count * inode_size / block_size)
    inode_table_block = 4
    next_data_block = inode_table_block + inode_table_blocks

    for node in nodes:
        if node.is_dir:
            node.size = block_size
            node.blocks = 1
        else:
            node.blocks = math.ceil(node.size / block_size) if node.size else 0
        node.indirect_block = 0
        if not use_extents and node.blocks > 12:
            node.indirect_block = next_data_block
            next_data_block += 1
        if node.blocks:
            node.first_block = next_data_block
            next_data_block += node.blocks

    if next_data_block >= part_blocks - 16:
        raise SystemExit(f"{part['name']} is too small for requested ext4 files")

    def disk_offset(fs_block):
        return (part_start_lba + fs_block) * block_size

    def write_block(fs_block, data):
        if len(data) > block_size:
            raise SystemExit("internal error: ext4 block payload too large")
        f.seek(disk_offset(fs_block))
        f.write(data)
        if len(data) < block_size:
            f.write(bytes(block_size - len(data)))

    def dirent(inode, name, file_type, rec_len=None):
        raw = name.encode("utf-8")
        actual_len = 8 + len(raw)
        padded = (actual_len + 3) & ~3
        rec_len = rec_len or padded
        entry = bytearray(rec_len)
        write_u32(entry, 0, inode)
        write_u16(entry, 4, rec_len)
        entry[6] = len(raw)
        entry[7] = file_type
        entry[8 : 8 + len(raw)] = raw
        return bytes(entry)

    def write_directory(node):
        entries = [
            dirent(node.inode, ".", EXT4_FT_DIR),
            dirent(node.parent.inode if node.parent else node.inode, "..", EXT4_FT_DIR),
        ]
        for child in node.children:
            file_type = EXT4_FT_DIR if child.is_dir else EXT4_FT_REG_FILE
            entries.append(dirent(child.inode, child.name, file_type))
        used = sum(len(entry) for entry in entries)
        if used > block_size:
            raise SystemExit(f"ext4 directory is too large: {node.name or '/'}")
        last = entries.pop()
        raw_name_len = last[6]
        compact_len = (8 + raw_name_len + 3) & ~3
        final_len = block_size - (used - len(last))
        final = bytearray(last[:compact_len] + bytes(final_len - compact_len))
        write_u16(final, 4, final_len)
        entries.append(bytes(final))
        write_block(node.first_block, b"".join(entries))

    def write_file(node):
        if node.size == 0:
            return
        if node.indirect_block:
            indirect = bytearray(block_size)
            for block_index in range(12, node.blocks):
                write_u32(indirect, (block_index - 12) * 4, node.first_block + block_index)
            write_block(node.indirect_block, indirect)
        f.seek(disk_offset(node.first_block))
        if node.source is not None:
            with open(node.source, "rb") as src:
                remaining = node.size
                while remaining > 0:
                    chunk = src.read(min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    f.write(chunk)
                    remaining -= len(chunk)
        else:
            f.write(node.data or b"")
        padding = node.blocks * block_size - node.size
        if padding:
            f.write(bytes(padding))

    def inode_bytes(node):
        inode = bytearray(inode_size)
        mode = 0x4000 | 0o755 if node.is_dir else 0x8000 | 0o644
        write_u16(inode, 0, mode)
        write_u32(inode, 4, node.size & 0xFFFFFFFF)
        now = int(time.time())
        for offset in (8, 12, 16):
            write_u32(inode, offset, now)
        write_u16(inode, 26, 2 if node.is_dir else 1)
        allocated_blocks = node.blocks + (1 if node.indirect_block else 0)
        write_u32(inode, 28, allocated_blocks * (block_size // 512))
        write_u32(inode, 32, EXT4_EXTENTS_FL if use_extents else 0)
        write_u32(inode, 108, node.size >> 32)
        if use_extents:
            write_u16(inode, 40, 0xF30A)
            write_u16(inode, 42, 1 if node.blocks else 0)
            write_u16(inode, 44, 4)
            write_u16(inode, 46, 0)
            if node.blocks:
                write_u32(inode, 52, 0)
                write_u16(inode, 56, node.blocks)
                write_u16(inode, 58, (node.first_block >> 32) & 0xFFFF)
                write_u32(inode, 60, node.first_block & 0xFFFFFFFF)
        else:
            for block_index in range(min(node.blocks, 12)):
                write_u32(inode, 40 + block_index * 4, node.first_block + block_index)
            if node.indirect_block:
                write_u32(inode, 40 + 12 * 4, node.indirect_block)
        return bytes(inode)

    superblock = bytearray(1024)
    write_u32(superblock, 0, inode_count)
    write_u32(superblock, 4, part_blocks)
    write_u32(superblock, 12, max(0, part_blocks - next_data_block))
    write_u32(superblock, 20, 0)
    write_u32(superblock, 24, 2)
    write_u32(superblock, 32, part_blocks)
    write_u32(superblock, 40, inode_count)
    write_u16(superblock, 56, 0xEF53)
    write_u32(superblock, 84, 11)
    write_u16(superblock, 88, inode_size)
    write_u32(superblock, 92, 0)
    write_u32(superblock, 96, 0x40 if use_extents else 0)
    write_u32(superblock, 100, 0)
    write_u32(superblock, 104, 0)
    write_u16(superblock, 254, 32)
    block0 = bytearray(block_size)
    block0[1024 : 1024 + len(superblock)] = superblock
    write_block(0, block0)

    group = bytearray(block_size)
    write_u32(group, 0, 2)
    write_u32(group, 4, 3)
    write_u32(group, 8, inode_table_block)
    write_block(1, group)
    write_block(2, bytes(block_size))
    write_block(3, bytes(block_size))

    inode_table = bytearray(inode_table_blocks * block_size)
    for node in nodes:
        offset = (node.inode - 1) * inode_size
        inode_table[offset : offset + inode_size] = inode_bytes(node)
    f.seek(disk_offset(inode_table_block))
    f.write(inode_table)

    for node in nodes:
        if node.is_dir:
            write_directory(node)
        else:
            write_file(node)
