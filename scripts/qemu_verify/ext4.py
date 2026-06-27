"""Minimal ext-family reader used by verify-qemu-image."""

from __future__ import annotations

from .common import FileExtent, FileRecord, Partition, VerifyError, require, u16, u32


EXT4_EXTENTS_FL = 0x00080000


class Ext4Volume:
    fs_type = "ext4"

    def __init__(self, image, partition: Partition):
        self.image = image
        self.partition = partition
        self.block_size = image.sector_size
        require(self.block_size == 4096, f"{partition.name}: verifier expects 4096 byte ext blocks")
        block0 = self.read_block(0)
        superblock = block0[1024:2048]
        require(u16(superblock, 56) == 0xEF53, f"{partition.name}: missing ext4 signature")
        require(1024 << u32(superblock, 24) == self.block_size, f"{partition.name}: ext block size mismatch")
        self.inode_size = u16(superblock, 88)
        self.inodes_per_group = u32(superblock, 40)
        self.group_desc_size = max(32, u16(superblock, 254))
        require(self.inode_size >= 128, f"{partition.name}: invalid ext4 inode size")
        require(self.inodes_per_group > 0, f"{partition.name}: invalid ext4 inode geometry")
        self.group_desc_block = 1

    def read_block(self, fs_block: int) -> bytes:
        return self.image.read_blocks(self.partition.start_lba + fs_block)

    def inode_table_block(self, group: int) -> int:
        desc_offset = group * self.group_desc_size
        block = self.read_block(self.group_desc_block + desc_offset // self.block_size)
        return u32(block, desc_offset % self.block_size + 8)

    def read_inode(self, inode_number: int) -> bytes:
        require(inode_number > 0, f"{self.partition.name}: invalid ext4 inode")
        group = (inode_number - 1) // self.inodes_per_group
        index = (inode_number - 1) % self.inodes_per_group
        inode_table = self.inode_table_block(group)
        offset = index * self.inode_size
        block = self.read_block(inode_table + offset // self.block_size)
        start = offset % self.block_size
        return block[start : start + self.inode_size]

    def inode_size_bytes(self, inode: bytes) -> int:
        return u32(inode, 4) | (u32(inode, 108) << 32)

    def is_dir(self, inode: bytes) -> bool:
        return u16(inode, 0) & 0xF000 == 0x4000

    def is_file(self, inode: bytes) -> bool:
        return u16(inode, 0) & 0xF000 == 0x8000

    def extents_for_inode(self, inode: bytes) -> list[FileExtent]:
        if not u32(inode, 32) & EXT4_EXTENTS_FL:
            return self.legacy_extents_for_inode(inode)
        root = inode[40:100]
        require(u16(root, 0) == 0xF30A, f"{self.partition.name}: invalid ext4 extent header")
        require(u16(root, 6) == 0, f"{self.partition.name}: indexed ext4 extents are unsupported")
        extents: list[FileExtent] = []
        for index in range(u16(root, 2)):
            offset = 12 + index * 12
            file_block = u32(root, offset)
            block_count = u16(root, offset + 4) & 0x7FFF
            physical = (u16(root, offset + 6) << 32) | u32(root, offset + 8)
            if block_count:
                extents.append(FileExtent(file_block, self.partition.start_lba + physical, block_count))
        return extents

    def legacy_extents_for_inode(self, inode: bytes) -> list[FileExtent]:
        needed = (self.inode_size_bytes(inode) + self.block_size - 1) // self.block_size
        if needed == 0:
            return []
        if needed > 12 + self.block_size // 4:
            raise VerifyError(f"{self.partition.name}: ext legacy block map is too large")
        raw_blocks = inode[40:100]
        blocks: list[int] = []
        for index in range(min(needed, 12)):
            blocks.append(u32(raw_blocks, index * 4))
        if needed > 12:
            indirect = u32(raw_blocks, 12 * 4)
            require(indirect != 0, f"{self.partition.name}: missing ext indirect block")
            indirect_block = self.read_block(indirect)
            for index in range(needed - 12):
                blocks.append(u32(indirect_block, index * 4))
        extents: list[FileExtent] = []
        for file_block, physical in enumerate(blocks):
            require(physical != 0, f"{self.partition.name}: sparse ext files are unsupported")
            if extents and extents[-1].physical_lba + extents[-1].block_count == self.partition.start_lba + physical:
                extents[-1].block_count += 1
            else:
                extents.append(FileExtent(file_block, self.partition.start_lba + physical, 1))
        return extents

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        inode = self.read_inode(record.first_cluster)
        require(self.is_file(inode), f"{record.name}: ext4 record is not a file")
        return self.extents_for_inode(inode)

    def read_directory(self, inode_number: int) -> list[FileRecord]:
        inode = self.read_inode(inode_number)
        require(self.is_dir(inode), f"{self.partition.name}: ext4 inode is not a directory")
        size = self.inode_size_bytes(inode)
        extents = self.extents_for_inode(inode)
        require(extents, f"{self.partition.name}: ext4 directory has no extents")
        data = self.image.read_at(extents[0].physical_lba * self.image.sector_size, size)
        out: list[FileRecord] = []
        offset = 0
        while offset + 8 <= len(data):
            inode_no = u32(data, offset)
            rec_len = u16(data, offset + 4)
            name_len = data[offset + 6]
            file_type = data[offset + 7]
            if rec_len < 8 or offset + rec_len > len(data):
                raise VerifyError(f"{self.partition.name}: invalid ext4 dirent")
            if inode_no:
                name = data[offset + 8 : offset + 8 + name_len].decode("utf-8")
                if name not in (".", ".."):
                    child = self.read_inode(inode_no)
                    out.append(FileRecord(name, file_type == 2 or self.is_dir(child), self.inode_size_bytes(child), inode_no, True))
            offset += rec_len
        return out

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.replace("\\", "/").split("/") if part]
        record = FileRecord("/", True, 0, 2, True)
        for index, part in enumerate(parts):
            for entry in self.read_directory(record.first_cluster):
                if entry.name.lower() == part.lower():
                    record = entry
                    break
            else:
                raise VerifyError(f"{self.partition.name}: missing ext4 path /{'/'.join(parts[:index + 1])}")
        return record
