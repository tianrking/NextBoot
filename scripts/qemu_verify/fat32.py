"""FAT32 reader used by verify-qemu-image."""

from __future__ import annotations

from .common import (
    FAT_EOC,
    FileExtent,
    FileRecord,
    Partition,
    DiskImage,
    append_extent,
    ceil_div,
    decode_fat_lfn,
    decode_short_name,
    require,
    u16,
    u32,
)

class Fat32Volume:
    fs_type = "fat32"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing FAT32 boot signature")
        require(self.boot[82:90] == b"FAT32   ", f"{partition.name}: missing FAT32 type marker")

        self.bytes_per_sector = u16(self.boot, 11)
        self.sectors_per_cluster = self.boot[13]
        self.reserved_sectors = u16(self.boot, 14)
        self.num_fats = self.boot[16]
        total16 = u16(self.boot, 19)
        total32 = u32(self.boot, 32)
        self.total_sectors = total16 or total32
        fat16 = u16(self.boot, 22)
        fat32 = u32(self.boot, 36)
        self.fat_size = fat16 or fat32
        self.root_cluster = u32(self.boot, 44)

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: FAT32 sector size mismatch")
        require(self.sectors_per_cluster > 0, f"{partition.name}: invalid FAT32 cluster size")
        require(self.total_sectors <= partition.block_count, f"{partition.name}: FAT32 volume exceeds partition")

        self.fat_lba = partition.start_lba + self.reserved_sectors
        self.data_lba = partition.start_lba + self.reserved_sectors + self.num_fats * self.fat_size

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def cluster_to_lba(self, cluster: int) -> int:
        return self.data_lba + (cluster - 2) * self.sectors_per_cluster

    def read_cluster(self, cluster: int) -> bytes:
        require(cluster >= 2, f"{self.partition.name}: invalid FAT32 cluster {cluster}")
        return self.image.read_blocks(self.cluster_to_lba(cluster), self.sectors_per_cluster)

    def next_cluster(self, cluster: int) -> int:
        offset = (self.fat_lba * self.image.sector_size) + cluster * 4
        return u32(self.image.read_at(offset, 4), 0) & 0x0FFFFFFF

    def cluster_chain(self, start_cluster: int) -> list[int]:
        require(start_cluster >= 2, f"{self.partition.name}: invalid FAT32 chain start")
        out: list[int] = []
        cluster = start_cluster
        while True:
            require(cluster not in out, f"{self.partition.name}: FAT32 cluster loop")
            out.append(cluster)
            nxt = self.next_cluster(cluster)
            if nxt >= FAT_EOC:
                return out
            require(nxt >= 2, f"{self.partition.name}: invalid FAT32 next cluster")
            cluster = nxt

    def read_directory(self, cluster: int) -> list[FileRecord]:
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

            first_cluster = (u16(entry, 20) << 16) | u16(entry, 26)
            records.append(
                FileRecord(
                    name=name,
                    is_dir=bool(attr & 0x10),
                    size=u32(entry, 28),
                    first_cluster=first_cluster,
                    contiguous=False,
                )
            )
        return records

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty FAT32 lookup")
        cluster = self.root_cluster
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(cluster)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing FAT32 path /{'/'.join(parts[:index + 1])}")
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
        require(blocks_remaining == 0, f"{record.name}: FAT32 file chain is too short")
        return extents
