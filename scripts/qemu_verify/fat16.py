"""FAT16 reader used by verify-qemu-image."""

from __future__ import annotations

from .common import (
    FileExtent,
    FileRecord,
    Partition,
    DiskImage,
    VerifyError,
    append_extent,
    ceil_div,
    decode_fat_lfn,
    decode_short_name,
    require,
    u16,
    u32,
)


class Fat16Volume:
    fs_type = "fat16"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing FAT16 boot signature")
        require(self.boot[54:62] == b"FAT16   ", f"{partition.name}: missing FAT16 type marker")

        self.bytes_per_sector = u16(self.boot, 11)
        self.sectors_per_cluster = self.boot[13]
        self.reserved_sectors = u16(self.boot, 14)
        self.num_fats = self.boot[16]
        self.root_entry_count = u16(self.boot, 17)
        total16 = u16(self.boot, 19)
        total32 = u32(self.boot, 32)
        self.total_sectors = total16 or total32
        self.fat_size = u16(self.boot, 22)

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: FAT16 sector size mismatch")
        require(self.sectors_per_cluster > 0, f"{partition.name}: invalid FAT16 cluster size")
        require(self.total_sectors <= partition.block_count, f"{partition.name}: FAT16 volume exceeds partition")
        require(self.fat_size > 0, f"{partition.name}: invalid FAT16 table size")

        self.root_dir_sectors = ceil_div(self.root_entry_count * 32, image.sector_size)
        self.fat_lba = partition.start_lba + self.reserved_sectors
        self.root_dir_lba = self.fat_lba + self.num_fats * self.fat_size
        self.data_lba = self.root_dir_lba + self.root_dir_sectors

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def cluster_to_lba(self, cluster: int) -> int:
        return self.data_lba + (cluster - 2) * self.sectors_per_cluster

    def read_cluster(self, cluster: int) -> bytes:
        require(cluster >= 2, f"{self.partition.name}: invalid FAT16 cluster {cluster}")
        return self.image.read_blocks(self.cluster_to_lba(cluster), self.sectors_per_cluster)

    def next_cluster(self, cluster: int) -> int:
        offset = (self.fat_lba * self.image.sector_size) + cluster * 2
        return u16(self.image.read_at(offset, 2), 0)

    def cluster_chain(self, start_cluster: int) -> list[int]:
        require(start_cluster >= 2, f"{self.partition.name}: invalid FAT16 chain start")
        out: list[int] = []
        cluster = start_cluster
        while True:
            require(cluster not in out, f"{self.partition.name}: FAT16 cluster loop")
            out.append(cluster)
            nxt = self.next_cluster(cluster)
            if nxt >= 0xFFF8:
                return out
            require(nxt >= 2, f"{self.partition.name}: invalid FAT16 next cluster")
            cluster = nxt

    def read_directory(self, cluster: int) -> list[FileRecord]:
        if cluster == 0:
            data = self.image.read_blocks(self.root_dir_lba, self.root_dir_sectors)
        else:
            data = b"".join(self.read_cluster(item) for item in self.cluster_chain(cluster))

        records: list[FileRecord] = []
        lfn_parts: dict[int, str] = {}
        for offset in range(0, len(data), 32):
            entry = data[offset : offset + 32]
            if len(entry) < 32:
                break
            first = entry[0]
            if first == 0:
                break
            if first == 0xE5:
                lfn_parts.clear()
                continue
            attr = entry[11]
            if attr == 0x0F:
                lfn_parts[first & 0x1F] = decode_fat_lfn(entry)
                continue
            if attr & 0x08:
                lfn_parts.clear()
                continue

            if lfn_parts:
                name = "".join(lfn_parts[index] for index in sorted(lfn_parts))
            else:
                name = decode_short_name(entry[:11])
            lfn_parts.clear()

            records.append(
                FileRecord(
                    name=name,
                    is_dir=bool(attr & 0x10),
                    size=u32(entry, 28),
                    first_cluster=u16(entry, 26),
                    contiguous=False,
                )
            )
        return records

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty FAT16 lookup")
        cluster = 0
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(cluster)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing FAT16 path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                cluster = record.first_cluster
        return record

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        blocks_remaining = ceil_div(record.size, self.image.sector_size)
        virtual_block = 0
        extents: list[FileExtent] = []
        for cluster in self.cluster_chain(record.first_cluster):
            block_count = min(self.cluster_blocks, blocks_remaining)
            append_extent(extents, virtual_block, self.cluster_to_lba(cluster), block_count)
            virtual_block += block_count
            blocks_remaining -= block_count
            if blocks_remaining == 0:
                break
        require(blocks_remaining == 0, f"{record.name}: FAT16 file chain is too short")
        return extents
