"""Minimal Btrfs smoke-volume reader used by verify-qemu-image."""

from __future__ import annotations

from .common import FileExtent, FileRecord, Partition, VerifyError, require, u16, u32, u64


BTRFS_SUPER_OFFSET = 64 * 1024
NEXTBOOT_SUPER_MAGIC = b"NXBTRFS1"
NEXTBOOT_NODE_MAGIC = b"NXBI1"
NEXTBOOT_DIR_MAGIC = b"NXBD1"
NODE_KIND_DIR = 1
NODE_KIND_FILE = 2


class BtrfsVolume:
    fs_type = "btrfs"

    def __init__(self, image, partition: Partition):
        self.image = image
        self.partition = partition
        superblock = image.read_at(partition.start_lba * image.sector_size + BTRFS_SUPER_OFFSET, image.sector_size)
        require(superblock[0x40:0x48] == b"_BHRfS_M", f"{partition.name}: missing Btrfs signature")
        require(superblock[0x100:0x108] == NEXTBOOT_SUPER_MAGIC, f"{partition.name}: unsupported Btrfs layout")
        self.block_size = u32(superblock, 0x108)
        require(self.block_size >= image.sector_size, f"{partition.name}: invalid Btrfs block size")
        require(self.block_size % image.sector_size == 0, f"{partition.name}: Btrfs block size is unaligned")
        self.blocks_per_fs_block = self.block_size // image.sector_size
        self.root_node = u64(superblock, 0x110)
        total_blocks = u64(superblock, 0x118)
        require(total_blocks * self.blocks_per_fs_block <= partition.block_count, f"{partition.name}: Btrfs geometry exceeds partition")
        require(self._read_node(self.root_node).is_dir, f"{partition.name}: Btrfs root is not a directory")

    def read_block(self, fs_block: int) -> bytes:
        offset = self.partition.start_lba * self.image.sector_size + fs_block * self.block_size
        return self.image.read_at(offset, self.block_size)

    def _read_node(self, node_id: int) -> FileRecord:
        block = self.read_block(node_id)
        require(block[0:5] == NEXTBOOT_NODE_MAGIC, f"{self.partition.name}: invalid Btrfs node")
        kind = block[8]
        size = u64(block, 16)
        first_block = u64(block, 24)
        return FileRecord(str(node_id), kind == NODE_KIND_DIR, size, node_id, True)

    def _node_layout(self, node_id: int) -> tuple[int, int, int]:
        block = self.read_block(node_id)
        require(block[0:5] == NEXTBOOT_NODE_MAGIC, f"{self.partition.name}: invalid Btrfs node")
        return block[8], u64(block, 24), u64(block, 32)

    def read_directory(self, node_id: int) -> list[FileRecord]:
        kind, first_block, blocks = self._node_layout(node_id)
        require(kind == NODE_KIND_DIR, f"{self.partition.name}: Btrfs node is not a directory")
        require(blocks == 1, f"{self.partition.name}: unsupported Btrfs directory extent count")
        data = self.read_block(first_block)
        require(data[0:5] == NEXTBOOT_DIR_MAGIC, f"{self.partition.name}: invalid Btrfs directory")
        count = u16(data, 6)
        entries: list[FileRecord] = []
        offset = 8
        for _ in range(count):
            child_id = u64(data, offset)
            name_len = data[offset + 8]
            offset += 9
            name = data[offset : offset + name_len].decode("utf-8")
            offset += name_len
            child = self._read_node(child_id)
            entries.append(FileRecord(name, child.is_dir, child.size, child_id, True))
        return entries

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.replace("\\", "/").split("/") if part]
        record = FileRecord("/", True, 0, self.root_node, True)
        for index, part in enumerate(parts):
            for entry in self.read_directory(record.first_cluster):
                if entry.name.lower() == part.lower():
                    record = entry
                    break
            else:
                raise VerifyError(f"{self.partition.name}: missing Btrfs path /{'/'.join(parts[:index + 1])}")
        return record

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        kind, first_block, blocks = self._node_layout(record.first_cluster)
        require(kind == NODE_KIND_FILE, f"{record.name}: Btrfs record is not a file")
        if blocks == 0:
            return []
        return [
            FileExtent(
                0,
                self.partition.start_lba + first_block * self.blocks_per_fs_block,
                blocks * self.blocks_per_fs_block,
            )
        ]
