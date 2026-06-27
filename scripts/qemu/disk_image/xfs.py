import math
import os
import struct
import time
import uuid


XFS_DINODE_FMT_EXTENTS = 2
XFS_DINODE_MAGIC = 0x494E
NEXTBOOT_XFS_DIR_MAGIC = b"NXD1"


class XfsNode:
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
    struct.pack_into(">H", data, offset, value)


def write_u32(data, offset, value):
    struct.pack_into(">I", data, offset, value)


def write_u64(data, offset, value):
    struct.pack_into(">Q", data, offset, value)


def write_xfs_volume(f, part, deps):
    sector_size = deps["sector_size"]
    block_size = 4096
    if block_size % sector_size != 0:
        raise SystemExit("test XFS block size must be aligned to the device sector size")

    efi_file = deps["efi_file"]
    efi_boot_name = deps["efi_boot_name"]
    smoke_vlnk_iso = deps["smoke_vlnk_iso"]
    image_files = deps["image_files"]
    extra_files = deps["extra_files"]
    split_virtual_path = deps["split_virtual_path"]

    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    part_blocks = part_sectors * sector_size // block_size
    if part_blocks < 256:
        raise SystemExit("partition is too small for XFS")

    root = XfsNode("", True)

    def ensure_dir(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = XfsNode(component, True, parent=current)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"XFS path component is not a directory: {component}")
        return current

    def add_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_dir("/" + "/".join(parts[:-1]))
        node = XfsNode(parts[-1], False, parent=directory, source=source, data=data)
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

    def assign(node):
        nodes.append(node)
        for child in node.children:
            assign(child)

    assign(root)

    next_block = 4
    for node in nodes:
        node.inode = next_block
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
        raise SystemExit(f"{part['name']} is too small for requested XFS files")

    def disk_offset(fs_block):
        return part_start_lba * sector_size + fs_block * block_size

    def write_block(fs_block, data):
        if len(data) > block_size:
            raise SystemExit("internal error: XFS block payload too large")
        f.seek(disk_offset(fs_block))
        f.write(data)
        if len(data) < block_size:
            f.write(bytes(block_size - len(data)))

    def bmbt_record(file_block, physical, count):
        l0 = (file_block << 9) | ((physical >> 43) & 0x1FF)
        l1 = ((physical & ((1 << 43) - 1)) << 21) | count
        return struct.pack(">QQ", l0, l1)

    def write_directory(node):
        block = bytearray(block_size)
        block[0:4] = NEXTBOOT_XFS_DIR_MAGIC
        write_u16(block, 4, len(node.children))
        offset = 6
        for child in node.children:
            raw = child.name.encode("utf-8")
            if len(raw) > 255:
                raise SystemExit(f"XFS name too long: {child.name}")
            end = offset + 9 + len(raw)
            if end > block_size:
                raise SystemExit(f"XFS directory is too large: {node.name or '/'}")
            write_u64(block, offset, child.inode)
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

    def inode_bytes(node):
        inode = bytearray(block_size)
        mode = 0x4000 | 0o755 if node.is_dir else 0x8000 | 0o644
        write_u16(inode, 0, XFS_DINODE_MAGIC)
        write_u16(inode, 2, mode)
        inode[4] = 2
        inode[5] = XFS_DINODE_FMT_EXTENTS
        write_u32(inode, 12, 0)
        write_u32(inode, 16, 0)
        write_u32(inode, 20, 2 if node.is_dir else 1)
        now = int(time.time())
        for offset in (32, 40, 48):
            write_u32(inode, offset, now)
        write_u64(inode, 56, node.size)
        write_u64(inode, 64, node.blocks)
        write_u32(inode, 76, 1 if node.blocks else 0)
        if node.blocks:
            inode[100:116] = bmbt_record(0, node.first_block, node.blocks)
        return bytes(inode)

    superblock = bytearray(block_size)
    superblock[0:4] = b"XFSB"
    write_u32(superblock, 4, block_size)
    write_u64(superblock, 8, part_blocks)
    superblock[32:48] = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-xfs:{part['name']}").bytes
    write_u64(superblock, 56, root.inode)
    write_u32(superblock, 84, part_blocks)
    write_u32(superblock, 88, 1)
    write_u16(superblock, 100, 0x0004)
    write_u16(superblock, 102, sector_size)
    write_u16(superblock, 104, 256)
    write_u16(superblock, 106, block_size // 256)
    write_block(0, superblock)

    for node in nodes:
        write_block(node.inode, inode_bytes(node))
    for node in nodes:
        if node.is_dir:
            write_directory(node)
        else:
            write_file(node)
