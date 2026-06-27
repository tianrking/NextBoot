"""Minimal UDF reader used by verify-qemu-image."""

from __future__ import annotations

from .common import FileExtent, FileRecord, Partition, require, u16, u32, u64


class UdfVolume:
    fs_type = "udf"

    def __init__(self, image, partition: Partition):
        self.image = image
        self.partition = partition
        self.block_size = image.sector_size
        self.partition_start = 0
        self.root_icb = 0
        self.mount()

    def read_block(self, lba: int) -> bytes:
        return self.image.read_blocks(self.partition.start_lba + lba)

    def read_partition_block(self, block_num: int) -> bytes:
        return self.read_block(self.partition_start + block_num)

    def mount(self) -> None:
        anchor = None
        for lba in (256, 512):
            if lba >= self.partition.block_count:
                continue
            block = self.read_block(lba)
            if u16(block, 0) == 0x0002 and u32(block, 12) == lba:
                anchor = block
                break
        require(anchor is not None, f"{self.partition.name}: missing UDF anchor")

        vds_start = u32(anchor, 20)
        vds_blocks = max(1, (u32(anchor, 16) + self.block_size - 1) // self.block_size)
        root_hint = 0
        partition_number = None
        for lba in range(vds_start, vds_start + vds_blocks):
            block = self.read_block(lba)
            ident = u16(block, 0)
            if ident == 0x0005:
                partition_number = u16(block, 22)
                self.partition_start = u32(block, 188)
            elif ident == 0x0006:
                require(u32(block, 212) == self.block_size, f"{self.partition.name}: UDF block size mismatch")
                root_hint = u32(block, 252)
                require(block[440] == 1 and block[441] >= 6, f"{self.partition.name}: unsupported UDF partition map")
                require(partition_number is None or u16(block, 444) == partition_number, f"{self.partition.name}: UDF map mismatch")
            elif ident == 0x0008:
                break

        require(self.partition_start > 0, f"{self.partition.name}: missing UDF partition descriptor")
        fsd = self.read_partition_block(root_hint)
        require(u16(fsd, 0) == 0x0100, f"{self.partition.name}: missing UDF file set descriptor")
        self.root_icb = u32(fsd, 404)

    def read_icb(self, block_num: int) -> tuple[bytes, int]:
        block = self.read_partition_block(block_num)
        ident = u16(block, 0)
        require(ident in (0x0105, 0x010A), f"{self.partition.name}: invalid UDF file entry")
        return block, ident

    def node_info(self, block_num: int) -> tuple[bool, int, int, int, int]:
        entry, ident = self.read_icb(block_num)
        is_dir = entry[27] == 0x04
        is_file = entry[27] == 0x05
        require(is_dir or is_file, f"{self.partition.name}: unsupported UDF node type")
        if ident == 0x0105:
            size = u64(entry, 56)
            alloc_offset = 176 + u32(entry, 168)
            alloc_len = u32(entry, 172)
        else:
            size = u64(entry, 64)
            alloc_offset = 216 + u32(entry, 208)
            alloc_len = u32(entry, 212)
        return is_dir, size, u16(entry, 34), alloc_offset, alloc_len

    def decode_name(self, raw: bytes) -> str:
        require(raw, f"{self.partition.name}: empty UDF name")
        if raw[0] == 8:
            return raw[1:].decode("utf-8")
        if raw[0] == 16:
            return raw[1:].decode("utf-16-be")
        raise AssertionError(f"{self.partition.name}: unsupported UDF name compression {raw[0]}")

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        is_dir, size, flags, alloc_offset, alloc_len = self.node_info(record.first_cluster)
        require(not is_dir, f"{record.name}: expected UDF file")
        require(flags & 0x0007 == 0, f"{record.name}: unsupported UDF allocation descriptor")
        if size == 0:
            return []
        entry, _ = self.read_icb(record.first_cluster)
        extents: list[FileExtent] = []
        virtual_block = 0
        for offset in range(alloc_offset, alloc_offset + alloc_len, 8):
            raw_len = u32(entry, offset)
            length = raw_len & 0x3FFFFFFF
            extent_type = raw_len & 0xC0000000
            block_num = u32(entry, offset + 4)
            if length == 0:
                continue
            blocks = (length + self.block_size - 1) // self.block_size
            if extent_type == 0:
                extents.append(FileExtent(virtual_block, self.partition.start_lba + self.partition_start + block_num, blocks))
            virtual_block += blocks
        return extents

    def read_directory(self, block_num: int) -> list[FileRecord]:
        is_dir, size, flags, alloc_offset, alloc_len = self.node_info(block_num)
        require(is_dir, f"{self.partition.name}: UDF node is not a directory")
        require(flags & 0x0007 == 0, f"{self.partition.name}: unsupported UDF directory descriptor")
        entry, _ = self.read_icb(block_num)
        require(alloc_len >= 8, f"{self.partition.name}: UDF directory has no data")
        dir_block = u32(entry, alloc_offset + 4)
        data = self.read_partition_block(dir_block)

        records: list[FileRecord] = []
        offset = 0
        while offset + 38 <= size:
            require(u16(data, offset) == 0x0101, f"{self.partition.name}: invalid UDF FID")
            characteristics = data[offset + 18]
            name_len = data[offset + 19]
            icb_block = u32(data, offset + 24)
            imp_use_len = u16(data, offset + 36)
            name_start = offset + 38 + imp_use_len
            name_end = name_start + name_len
            raw_name = data[name_start:name_end]
            if characteristics & (0x04 | 0x08) == 0:
                child_is_dir, child_size, _, _, _ = self.node_info(icb_block)
                records.append(FileRecord(self.decode_name(raw_name), child_is_dir, child_size, icb_block, True))
            offset = (name_end + 3) & ~3
        return records

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.replace("\\", "/").split("/") if part]
        record = FileRecord("/", True, 0, self.root_icb, True)
        for index, part in enumerate(parts):
            entries = self.read_directory(record.first_cluster)
            for entry in entries:
                if entry.name.lower() == part.lower():
                    record = entry
                    break
            else:
                raise AssertionError(f"{self.partition.name}: missing UDF path /{'/'.join(parts[:index + 1])}")
        return record
