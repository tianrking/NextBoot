"""Minimal XFS reader used by verify-qemu-image."""

from __future__ import annotations

import struct

from .common import FileExtent, FileRecord, Partition, VerifyError, require


NEXTBOOT_XFS_DIR_MAGIC = b"NXD1"
XFS_DINODE_FMT_EXTENTS = 2


class XfsVolume:
    fs_type = "xfs"

    def __init__(self, image, partition: Partition):
        self.image = image
        self.partition = partition
        superblock = image.read_blocks(partition.start_lba)
        require(superblock[0:4] == b"XFSB", f"{partition.name}: missing XFS signature")
        self.block_size = be32(superblock, 4)
        require(self.block_size >= image.sector_size, f"{partition.name}: XFS block is smaller than sector")
        require(self.block_size % image.sector_size == 0, f"{partition.name}: XFS block is not sector aligned")
        self.root_inode = be64(superblock, 56)
        self.inode_size = be16(superblock, 104)
        require(self.inode_size >= 128, f"{partition.name}: invalid XFS inode size")

    @property
    def sectors_per_block(self) -> int:
        return self.block_size // self.image.sector_size

    def read_block(self, fs_block: int) -> bytes:
        offset = self.partition.start_lba * self.image.sector_size + fs_block * self.block_size
        return self.image.read_at(offset, self.block_size)

    def read_inode(self, inode_number: int) -> bytes:
        require(inode_number > 0, f"{self.partition.name}: invalid XFS inode")
        inode = self.read_block(inode_number)
        require(be16(inode, 0) == 0x494E, f"{self.partition.name}: invalid XFS inode magic")
        require(inode[5] == XFS_DINODE_FMT_EXTENTS, f"{self.partition.name}: unsupported XFS inode format")
        return inode

    def inode_size_bytes(self, inode: bytes) -> int:
        return be64(inode, 56)

    def is_dir(self, inode: bytes) -> bool:
        return be16(inode, 2) & 0xF000 == 0x4000

    def is_file(self, inode: bytes) -> bool:
        return be16(inode, 2) & 0xF000 == 0x8000

    def extents_for_inode(self, inode: bytes) -> list[FileExtent]:
        count = be32(inode, 76)
        extents: list[FileExtent] = []
        for index in range(count):
            offset = 100 + index * 16
            l0 = be64(inode, offset)
            l1 = be64(inode, offset + 8)
            require(l0 >> 63 == 0, f"{self.partition.name}: unsupported XFS unwritten extent")
            file_block = (l0 >> 9) & ((1 << 54) - 1)
            physical = ((l0 & 0x1FF) << 43) | (l1 >> 21)
            block_count = l1 & ((1 << 21) - 1)
            require(block_count > 0, f"{self.partition.name}: empty XFS extent")
            extents.append(
                FileExtent(
                    file_block * self.sectors_per_block,
                    self.partition.start_lba + physical * self.sectors_per_block,
                    block_count * self.sectors_per_block,
                )
            )
        return extents

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        inode = self.read_inode(record.first_cluster)
        require(self.is_file(inode), f"{record.name}: XFS record is not a file")
        return self.extents_for_inode(inode)

    def read_directory(self, inode_number: int) -> list[FileRecord]:
        inode = self.read_inode(inode_number)
        require(self.is_dir(inode), f"{self.partition.name}: XFS inode is not a directory")
        extents = self.extents_for_inode(inode)
        require(extents, f"{self.partition.name}: XFS directory has no extents")
        data = self.image.read_at(extents[0].physical_lba * self.image.sector_size, self.inode_size_bytes(inode))
        require(data[0:4] == NEXTBOOT_XFS_DIR_MAGIC, f"{self.partition.name}: unsupported XFS directory block")
        out: list[FileRecord] = []
        offset = 6
        for _ in range(be16(data, 4)):
            inode_no = be64(data, offset)
            name_len = data[offset + 8]
            offset += 9
            name = data[offset : offset + name_len].decode("utf-8")
            offset += name_len
            child = self.read_inode(inode_no)
            out.append(FileRecord(name, self.is_dir(child), self.inode_size_bytes(child), inode_no, True))
        return out

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.replace("\\", "/").split("/") if part]
        record = FileRecord("/", True, 0, self.root_inode, True)
        for index, part in enumerate(parts):
            for entry in self.read_directory(record.first_cluster):
                if entry.name.lower() == part.lower():
                    record = entry
                    break
            else:
                raise VerifyError(f"{self.partition.name}: missing XFS path /{'/'.join(parts[:index + 1])}")
        return record


def be16(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from(">H", data, offset)[0]


def be32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from(">I", data, offset)[0]


def be64(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from(">Q", data, offset)[0]
