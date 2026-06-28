"""exFAT reader used by verify-qemu-image."""

from __future__ import annotations

from .common import (
    EXFAT_EOC,
    FileExtent,
    FileRecord,
    Partition,
    DiskImage,
    VerifyError,
    append_extent,
    ceil_div,
    require,
    u16,
    u32,
    u64,
)

class ExFatVolume:
    fs_type = "exfat"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        self.main_boot_region = image.read_blocks(partition.start_lba, 12)
        self.backup_boot_region = image.read_blocks(partition.start_lba + 12, 12)
        require(self.boot[0:3] == b"\xeb\x76\x90", f"{partition.name}: missing exFAT jump")
        require(self.boot[3:11] == b"EXFAT   ", f"{partition.name}: missing exFAT marker")
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing exFAT boot signature")
        require(
            self.main_boot_region == self.backup_boot_region,
            f"{partition.name}: exFAT backup boot region mismatch",
        )
        require(
            self.boot_region_checksum_ok(self.main_boot_region),
            f"{partition.name}: invalid exFAT boot checksum",
        )
        for sector in range(1, 9):
            marker = self.main_boot_region[
                sector * image.sector_size + image.sector_size - 4 :
                (sector + 1) * image.sector_size
            ]
            require(
                marker == b"\x00\x00\x55\xaa",
                f"{partition.name}: missing exFAT extended boot signature in sector {sector}",
            )

        self.partition_offset = u64(self.boot, 64)
        self.volume_length = u64(self.boot, 72)
        self.fat_offset = u32(self.boot, 80)
        self.fat_length = u32(self.boot, 84)
        self.cluster_heap_offset = u32(self.boot, 88)
        self.cluster_count = u32(self.boot, 92)
        self.root_cluster = u32(self.boot, 96)
        self.bytes_per_sector = 1 << self.boot[108]
        self.sectors_per_cluster = 1 << self.boot[109]
        self.num_fats = self.boot[110]

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: exFAT sector size mismatch")
        require(self.num_fats == 1, f"{partition.name}: expected one exFAT FAT")
        require(self.partition_offset == partition.start_lba, f"{partition.name}: exFAT partition offset mismatch")
        require(self.volume_length <= partition.block_count, f"{partition.name}: exFAT volume exceeds partition")
        self.require_system_root_entries()

    def boot_region_checksum_ok(self, boot_region: bytes) -> bool:
        checksum_sector = 11 * self.image.sector_size
        checksum = 0
        for offset, byte in enumerate(boot_region[:checksum_sector]):
            if offset in (106, 107, 112):
                continue
            checksum = ((checksum >> 1) | ((checksum & 1) << 31)) & 0xFFFFFFFF
            checksum = (checksum + byte) & 0xFFFFFFFF
        expected = checksum.to_bytes(4, "little") * (self.image.sector_size // 4)
        return boot_region[checksum_sector : checksum_sector + self.image.sector_size] == expected

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def cluster_to_lba(self, cluster: int) -> int:
        return self.partition.start_lba + self.cluster_heap_offset + (cluster - 2) * self.sectors_per_cluster

    def read_cluster(self, cluster: int) -> bytes:
        require(2 <= cluster < self.cluster_count + 2, f"{self.partition.name}: invalid exFAT cluster {cluster}")
        return self.image.read_blocks(self.cluster_to_lba(cluster), self.sectors_per_cluster)

    def next_cluster(self, cluster: int) -> int:
        offset = (self.partition.start_lba + self.fat_offset) * self.image.sector_size + cluster * 4
        return u32(self.image.read_at(offset, 4), 0)

    def cluster_chain(self, start_cluster: int) -> list[int]:
        require(start_cluster >= 2, f"{self.partition.name}: invalid exFAT chain start")
        out: list[int] = []
        cluster = start_cluster
        while True:
            require(cluster not in out, f"{self.partition.name}: exFAT cluster loop")
            out.append(cluster)
            nxt = self.next_cluster(cluster)
            if nxt >= EXFAT_EOC:
                return out
            require(2 <= nxt < self.cluster_count + 2, f"{self.partition.name}: invalid exFAT next cluster")
            cluster = nxt

    def read_directory(self, cluster: int) -> list[FileRecord]:
        data = b"".join(self.read_cluster(item) for item in self.cluster_chain(cluster))
        records: list[FileRecord] = []
        offset = 0
        while offset + 32 <= len(data):
            entry_type = data[offset]
            if entry_type == 0:
                break
            if entry_type == 0x85:
                secondary_count = data[offset + 1]
                group = data[offset : offset + 32 * (secondary_count + 1)]
                require(len(group) == 32 * (secondary_count + 1), f"{self.partition.name}: truncated exFAT entry set")
                parsed = self.parse_entry_set(group)
                if parsed is not None:
                    records.append(parsed)
                offset += 32 * (secondary_count + 1)
            else:
                offset += 32
        return records

    def raw_directory_entries(self, cluster: int) -> list[bytes]:
        data = b"".join(self.read_cluster(item) for item in self.cluster_chain(cluster))
        entries: list[bytes] = []
        for offset in range(0, len(data), 32):
            entry = data[offset : offset + 32]
            if len(entry) < 32 or entry[0] == 0:
                break
            entries.append(entry)
        return entries

    def require_system_root_entries(self) -> None:
        entries = self.raw_directory_entries(self.root_cluster)
        entry_types = {entry[0] for entry in entries}
        require(0x81 in entry_types, f"{self.partition.name}: missing exFAT allocation bitmap entry")
        require(0x82 in entry_types, f"{self.partition.name}: missing exFAT upcase table entry")
        require(0x83 in entry_types, f"{self.partition.name}: missing exFAT volume label entry")

        bitmap = next(entry for entry in entries if entry[0] == 0x81)
        bitmap_cluster = u32(bitmap, 20)
        bitmap_size = u64(bitmap, 24)
        require(bitmap_cluster >= 2, f"{self.partition.name}: invalid exFAT bitmap cluster")
        require(bitmap_size >= ceil_div(self.cluster_count, 8), f"{self.partition.name}: exFAT bitmap is too small")

        upcase = next(entry for entry in entries if entry[0] == 0x82)
        upcase_cluster = u32(upcase, 20)
        upcase_size = u64(upcase, 24)
        require(upcase_cluster >= 2, f"{self.partition.name}: invalid exFAT upcase cluster")
        require(upcase_size > 0, f"{self.partition.name}: empty exFAT upcase table")

    def parse_entry_set(self, group: bytes) -> FileRecord | None:
        attr = u16(group, 4)
        if attr & 0x0006:
            return None

        first_cluster = 0
        size = 0
        name_length = 0
        name_chars: list[str] = []
        contiguous = False

        for offset in range(32, len(group), 32):
            entry = group[offset : offset + 32]
            if entry[0] == 0xC0:
                contiguous = bool(entry[1] & 0x02)
                name_length = entry[3]
                first_cluster = u32(entry, 20)
                size = u64(entry, 24)
            elif entry[0] == 0xC1:
                remaining = name_length - len(name_chars)
                for index in range(min(15, max(0, remaining))):
                    value = u16(entry, 2 + index * 2)
                    if value == 0:
                        break
                    name_chars.append(chr(value))

        return FileRecord(
            name="".join(name_chars),
            is_dir=bool(attr & 0x0010),
            size=size,
            first_cluster=first_cluster,
            contiguous=contiguous,
        )

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty exFAT lookup")
        cluster = self.root_cluster
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(cluster)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing exFAT path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                cluster = record.first_cluster
        return record

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        blocks_remaining = ceil_div(record.size, self.image.sector_size)
        if record.contiguous:
            return [FileExtent(0, self.cluster_to_lba(record.first_cluster), blocks_remaining)]

        virtual_block = 0
        extents: list[FileExtent] = []
        for cluster in self.cluster_chain(record.first_cluster):
            block_count = min(self.cluster_blocks, blocks_remaining)
            append_extent(extents, virtual_block, self.cluster_to_lba(cluster), block_count)
            virtual_block += block_count
            blocks_remaining -= block_count
            if blocks_remaining == 0:
                break
        require(blocks_remaining == 0, f"{record.name}: exFAT file chain is too short")
        return extents
