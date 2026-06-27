"""NTFS reader used by verify-qemu-image."""

from __future__ import annotations

from .common import (
    NTFS_ATTR_TYPE_DATA,
    NTFS_ATTR_TYPE_END,
    NTFS_ATTR_TYPE_INDEX_ROOT,
    NTFS_FILE_ATTRIBUTE_DIRECTORY,
    NTFS_INDEX_ENTRY_LAST,
    NTFS_OEM_ID,
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

class NtfsVolume:
    fs_type = "ntfs"

    def __init__(self, image: DiskImage, partition: Partition):
        self.image = image
        self.partition = partition
        self.boot = image.read_blocks(partition.start_lba)
        require(self.boot[3:11] == NTFS_OEM_ID, f"{partition.name}: missing NTFS marker")
        require(self.boot[510:512] == b"\x55\xaa", f"{partition.name}: missing NTFS boot signature")

        self.bytes_per_sector = u16(self.boot, 0x0B)
        self.sectors_per_cluster = self.boot[0x0D]
        self.total_sectors = u64(self.boot, 0x28)
        self.mft_lcn = u64(self.boot, 0x30)
        self.file_record_size = self.decode_record_size(self.boot[0x40])
        self.index_record_size = self.decode_record_size(self.boot[0x44])

        require(self.bytes_per_sector == image.sector_size, f"{partition.name}: NTFS sector size mismatch")
        require(self.sectors_per_cluster > 0, f"{partition.name}: invalid NTFS cluster size")
        require(self.total_sectors <= partition.block_count, f"{partition.name}: NTFS volume exceeds partition")
        require(self.file_record_size % image.sector_size == 0, f"{partition.name}: NTFS file record is not sector aligned")
        require(self.index_record_size % image.sector_size == 0, f"{partition.name}: NTFS index record is not sector aligned")

    @property
    def cluster_size(self) -> int:
        return self.bytes_per_sector * self.sectors_per_cluster

    @property
    def cluster_blocks(self) -> int:
        return self.sectors_per_cluster

    def decode_record_size(self, raw: int) -> int:
        value = raw if raw < 128 else raw - 256
        if value > 0:
            return value * self.bytes_per_sector * self.sectors_per_cluster
        require(value < 0, f"{self.partition.name}: invalid NTFS record size code")
        return 1 << (-value)

    def read_file_record(self, record_number: int) -> bytes:
        offset = (
            self.partition.start_lba * self.image.sector_size
            + self.mft_lcn * self.cluster_size
            + record_number * self.file_record_size
        )
        record = bytearray(self.image.read_at(offset, self.file_record_size))
        require(record[0:4] == b"FILE", f"{self.partition.name}: bad NTFS FILE record {record_number}")
        self.apply_fixup(record)
        return bytes(record)

    def apply_fixup(self, record: bytearray) -> None:
        usa_offset = u16(record, 4)
        usa_count = u16(record, 6)
        sector_count = len(record) // self.bytes_per_sector
        require(usa_count == sector_count + 1, f"{self.partition.name}: invalid NTFS update sequence count")
        require(usa_offset + usa_count * 2 <= len(record), f"{self.partition.name}: NTFS update sequence is out of range")
        sequence = u16(record, usa_offset)
        for sector in range(sector_count):
            tail = (sector + 1) * self.bytes_per_sector - 2
            require(u16(record, tail) == sequence, f"{self.partition.name}: NTFS update sequence mismatch")
            replacement = record[usa_offset + 2 * (sector + 1) : usa_offset + 2 * (sector + 2)]
            record[tail : tail + 2] = replacement

    def attributes(self, record: bytes) -> list[tuple[int, bytes]]:
        attrs_offset = u16(record, 0x14)
        require(attrs_offset < len(record), f"{self.partition.name}: NTFS attribute offset is out of range")
        out: list[tuple[int, bytes]] = []
        offset = attrs_offset
        while offset + 8 <= len(record):
            attr_type = u32(record, offset)
            if attr_type == NTFS_ATTR_TYPE_END:
                break
            attr_len = u32(record, offset + 4)
            require(attr_len > 0, f"{self.partition.name}: zero-length NTFS attribute")
            require(offset + attr_len <= len(record), f"{self.partition.name}: NTFS attribute exceeds record")
            out.append((attr_type, record[offset : offset + attr_len]))
            offset += attr_len
        return out

    def resident_value(self, attr: bytes) -> bytes:
        require(attr[8] == 0, f"{self.partition.name}: expected resident NTFS attribute")
        value_len = u32(attr, 0x10)
        value_offset = u16(attr, 0x14)
        require(value_offset + value_len <= len(attr), f"{self.partition.name}: NTFS resident value exceeds attribute")
        return attr[value_offset : value_offset + value_len]

    def parse_index_entries(self, data: bytes) -> list[FileRecord]:
        records: list[FileRecord] = []
        offset = 0
        while offset + 16 <= len(data):
            entry_len = u16(data, offset + 8)
            stream_len = u16(data, offset + 10)
            flags = u16(data, offset + 12)
            require(entry_len > 0, f"{self.partition.name}: zero-length NTFS index entry")
            require(offset + entry_len <= len(data), f"{self.partition.name}: NTFS index entry exceeds buffer")
            if flags & NTFS_INDEX_ENTRY_LAST:
                break
            stream_start = offset + 16
            stream_end = stream_start + stream_len
            require(stream_end <= offset + entry_len, f"{self.partition.name}: NTFS index stream exceeds entry")
            parsed = self.parse_file_name_entry(data[offset : offset + 6], data[stream_start:stream_end])
            if parsed is not None:
                records.append(parsed)
            offset += entry_len
        return records

    def parse_file_name_entry(self, record_ref: bytes, stream: bytes) -> FileRecord | None:
        if len(stream) < 66:
            return None
        namespace = stream[65]
        if namespace == 2:
            return None
        allocated_size = u64(stream, 40)
        real_size = u64(stream, 48)
        raw_flags = u32(stream, 56)
        name_len = stream[64]
        name_bytes = name_len * 2
        require(66 + name_bytes <= len(stream), f"{self.partition.name}: NTFS filename exceeds entry")
        name = stream[66 : 66 + name_bytes].decode("utf-16le", errors="strict")
        is_dir = bool(raw_flags & NTFS_FILE_ATTRIBUTE_DIRECTORY)
        return FileRecord(
            name=name,
            is_dir=is_dir,
            size=allocated_size if is_dir else real_size,
            first_cluster=int.from_bytes(record_ref + b"\x00\x00", "little"),
            contiguous=False,
        )

    def read_directory(self, record_number: int) -> list[FileRecord]:
        record = self.read_file_record(record_number)
        flags = u16(record, 0x16)
        require(flags & 0x0002, f"{self.partition.name}: NTFS record {record_number} is not a directory")
        for attr_type, attr in self.attributes(record):
            if attr_type != NTFS_ATTR_TYPE_INDEX_ROOT:
                continue
            value = self.resident_value(attr)
            require(len(value) >= 32, f"{self.partition.name}: NTFS index root is too small")
            index_header = 16
            entries_offset = u32(value, index_header)
            total_size = u32(value, index_header + 4)
            start = index_header + entries_offset
            end = min(index_header + total_size, len(value))
            require(start <= end, f"{self.partition.name}: invalid NTFS index range")
            return self.parse_index_entries(value[start:end])
        raise VerifyError(f"{self.partition.name}: NTFS directory record {record_number} has no index root")

    def lookup(self, path: str) -> FileRecord:
        parts = [part for part in path.strip("/").split("/") if part]
        require(parts, "empty NTFS lookup")
        record_number = 5
        record: FileRecord | None = None
        for index, part in enumerate(parts):
            entries = self.read_directory(record_number)
            record = next((item for item in entries if item.name.lower() == part.lower()), None)
            if record is None:
                raise VerifyError(f"{self.partition.name}: missing NTFS path /{'/'.join(parts[:index + 1])}")
            if index < len(parts) - 1:
                require(record.is_dir, f"{self.partition.name}: /{'/'.join(parts[:index + 1])} is not a directory")
                record_number = record.first_cluster
        return record

    def parse_data_runs(self, data: bytes) -> list[tuple[int, int, int | None]]:
        runs: list[tuple[int, int, int | None]] = []
        offset = 0
        current_vcn = 0
        current_lcn = 0
        while offset < len(data):
            header = data[offset]
            offset += 1
            if header == 0:
                break
            len_size = header & 0x0F
            off_size = header >> 4
            require(len_size > 0 and len_size <= 8 and off_size <= 8, f"{self.partition.name}: invalid NTFS run header")
            require(offset + len_size + off_size <= len(data), f"{self.partition.name}: truncated NTFS run")
            cluster_count = int.from_bytes(data[offset : offset + len_size], "little")
            offset += len_size
            require(cluster_count > 0, f"{self.partition.name}: zero-length NTFS run")
            if off_size:
                delta = int.from_bytes(data[offset : offset + off_size], "little", signed=True)
                offset += off_size
                current_lcn += delta
                require(current_lcn >= 0, f"{self.partition.name}: negative NTFS run LCN")
                lcn: int | None = current_lcn
            else:
                lcn = None
            runs.append((current_vcn, cluster_count, lcn))
            current_vcn += cluster_count
        return runs

    def file_extents(self, record: FileRecord) -> list[FileExtent]:
        require(not record.is_dir, f"{record.name} is a directory")
        if record.size == 0:
            return []
        file_record = self.read_file_record(record.first_cluster)
        data_attrs = [attr for attr_type, attr in self.attributes(file_record) if attr_type == NTFS_ATTR_TYPE_DATA]
        require(data_attrs, f"{record.name}: NTFS file has no data attribute")
        require(len(data_attrs) == 1, f"{record.name}: verifier supports one NTFS data attribute")
        attr = data_attrs[0]
        require(attr[8] != 0, f"{record.name}: verifier expects non-resident NTFS data")
        real_size = u64(attr, 0x30)
        require(real_size == record.size, f"{record.name}: NTFS data size mismatch")
        runlist_offset = u16(attr, 0x20)
        require(runlist_offset < len(attr), f"{record.name}: NTFS runlist offset is out of range")
        runs = self.parse_data_runs(attr[runlist_offset:])
        remaining_blocks = ceil_div(record.size, self.image.sector_size)
        extents: list[FileExtent] = []
        for virtual_cluster, cluster_count, lcn in runs:
            require(lcn is not None, f"{record.name}: sparse NTFS runs are not expected")
            run_blocks = cluster_count * self.cluster_blocks
            block_count = min(run_blocks, remaining_blocks)
            if block_count == 0:
                break
            append_extent(
                extents,
                virtual_cluster * self.cluster_blocks,
                self.partition.start_lba + lcn * self.cluster_blocks,
                block_count,
            )
            remaining_blocks -= block_count
            if remaining_blocks == 0:
                break
        require(remaining_blocks == 0, f"{record.name}: NTFS file runs are too short")
        return extents
