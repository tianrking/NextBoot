import math
import os
import struct


BTRFS_SUPER_OFFSET = 64 * 1024
NEXTBOOT_SUPER_MAGIC = b"NXBTRFS1"
NEXTBOOT_NODE_MAGIC = b"NXBI1"
NEXTBOOT_DIR_MAGIC = b"NXBD1"
NODE_KIND_DIR = 1
NODE_KIND_FILE = 2


class BtrfsNode:
    def __init__(self, name, is_dir, parent=None, source=None, data=None):
        self.name = name
        self.is_dir = is_dir
        self.parent = parent
        self.source = source
        self.data = data
        self.children = []
        self.children_by_name = {}
        self.node_id = 0
        self.size = 0
        self.first_block = 0
        self.blocks = 0


def write_u16(data, offset, value):
    struct.pack_into("<H", data, offset, value)


def write_u32(data, offset, value):
    struct.pack_into("<I", data, offset, value)


def write_u64(data, offset, value):
    struct.pack_into("<Q", data, offset, value)


def write_btrfs_volume(f, part, deps):
    sector_size = deps["sector_size"]
    block_size = 4096
    if block_size % sector_size != 0:
        raise SystemExit("test Btrfs block size must be aligned to the device sector size")

    efi_entries = deps["efi_entries"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]

    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    part_blocks = part_sectors * sector_size // block_size
    if part_blocks < 256:
        raise SystemExit("partition is too small for Btrfs")

    root = BtrfsNode("", True)

    def ensure_dir(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = BtrfsNode(component, True, parent=current)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"Btrfs path component is not a directory: {component}")
        return current

    def add_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_dir("/" + "/".join(parts[:-1]))
        node = BtrfsNode(parts[-1], False, parent=directory, source=source, data=data)
        node.size = os.path.getsize(source) if source is not None else len(data or b"")
        directory.children.append(node)
        directory.children_by_name[node.name.lower()] = node

    if part["include_efi"]:
        for efi_boot_name, efi_file in efi_entries:
            add_file(f"/EFI/BOOT/{efi_boot_name}", source=efi_file)

    if part["include_images"]:
        if not smoke_vlnk_iso:
            ensure_dir("/ISO")
            for image in image_files:
                add_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_file(virtual_path, data=data)

    nodes = []

    def assign(node):
        nodes.append(node)
        for child in node.children:
            assign(child)

    assign(root)

    next_block = max(32, BTRFS_SUPER_OFFSET // block_size + 1)
    for node in nodes:
        node.node_id = next_block
        next_block += 1
    next_data_block = next_block
    for node in nodes:
        if node.is_dir:
            node.size = block_size
            node.blocks = 1
        else:
            node.blocks = math.ceil(node.size / block_size) if node.size else 0
        if node.blocks:
            node.first_block = next_data_block
            next_data_block += node.blocks

    if next_data_block >= part_blocks - 16:
        raise SystemExit(f"{part['name']} is too small for requested Btrfs files")

    def disk_offset(fs_block):
        return part_start_lba * sector_size + fs_block * block_size

    def write_block(fs_block, data):
        if len(data) > block_size:
            raise SystemExit("internal error: Btrfs block payload too large")
        f.seek(disk_offset(fs_block))
        f.write(data)
        if len(data) < block_size:
            f.write(bytes(block_size - len(data)))

    def write_node(node):
        block = bytearray(block_size)
        block[0:5] = NEXTBOOT_NODE_MAGIC
        block[8] = NODE_KIND_DIR if node.is_dir else NODE_KIND_FILE
        write_u64(block, 16, node.size)
        write_u64(block, 24, node.first_block)
        write_u64(block, 32, node.blocks)
        write_block(node.node_id, block)

    def write_directory(node):
        block = bytearray(block_size)
        block[0:5] = NEXTBOOT_DIR_MAGIC
        write_u16(block, 6, len(node.children))
        offset = 8
        for child in node.children:
            raw = child.name.encode("utf-8")
            if len(raw) > 255:
                raise SystemExit(f"Btrfs name too long: {child.name}")
            end = offset + 9 + len(raw)
            if end > block_size:
                raise SystemExit(f"Btrfs directory is too large: {node.name or '/'}")
            write_u64(block, offset, child.node_id)
            block[offset + 8] = len(raw)
            block[offset + 9 : end] = raw
            offset = end
        write_block(node.first_block, block)

    def write_file(node):
        if node.size == 0:
            return
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

    superblock = bytearray(block_size)
    superblock[0x40:0x48] = b"_BHRfS_M"
    superblock[0x100:0x108] = NEXTBOOT_SUPER_MAGIC
    write_u32(superblock, 0x108, block_size)
    write_u64(superblock, 0x110, root.node_id)
    write_u64(superblock, 0x118, part_blocks)
    f.seek(part_start_lba * sector_size + BTRFS_SUPER_OFFSET)
    f.write(superblock)

    for node in nodes:
        write_node(node)
    for node in nodes:
        if node.is_dir:
            write_directory(node)
        else:
            write_file(node)
