//! Minimal read-only NTFS filesystem support.
//!
//! This implements the subset needed for Ventoy-style data partitions: walking
//! normal directory indexes, reading resident and non-resident unnamed `$DATA`,
//! and exposing non-resident runlists as block extents.

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::String;
use alloc::vec::Vec;

const NTFS_OEM_ID: &[u8; 8] = b"NTFS    ";
const FILE_RECORD_MAGIC: &[u8; 4] = b"FILE";
const INDEX_RECORD_MAGIC: &[u8; 4] = b"INDX";

const MFT_RECORD_ROOT: u64 = 5;

#[cfg(test)]
const ATTR_TYPE_FILE_NAME: u32 = 0x30;
const ATTR_TYPE_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_TYPE_DATA: u32 = 0x80;
const ATTR_TYPE_INDEX_ROOT: u32 = 0x90;
const ATTR_TYPE_INDEX_ALLOCATION: u32 = 0xA0;
const ATTR_TYPE_END: u32 = 0xFFFF_FFFF;

const FILE_FLAG_IN_USE: u16 = 0x0001;
const FILE_FLAG_DIRECTORY: u16 = 0x0002;

const INDEX_ENTRY_HAS_CHILD: u16 = 0x0001;
const INDEX_ENTRY_LAST: u16 = 0x0002;
const INDEX_HEADER_HAS_ALLOCATION: u8 = 0x01;

const FILE_ATTRIBUTE_READ_ONLY: u32 = 0x0000_0001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x1000_0000;

const FILE_NAME_NAMESPACE_DOS: u8 = 2;

/// Read-only NTFS filesystem.
pub struct Ntfs {
    block_io: SharedBlockIo,
    bytes_per_sector: u32,
    sectors_per_cluster: u8,
    cluster_size: u64,
    total_sectors: u64,
    mft_lcn: u64,
    file_record_size: u32,
    index_record_size: u32,
    mft_runs: Vec<DataRun>,
}

impl FileSystem for Ntfs {
    const FS_TYPE: FileSystemType = FileSystemType::Ntfs;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let mut boot = alloc_buffer(block_io.block_size() as usize)?;
        read_full_blocks(block_io.as_ref(), 0, &mut boot)?;

        if boot.len() < 512 || &boot[3..11] != NTFS_OEM_ID || boot[510] != 0x55 || boot[511] != 0xAA
        {
            return Err(FsError::InvalidSignature);
        }

        let bytes_per_sector = read_u16(&boot, 0x0B)? as u32;
        if bytes_per_sector == 0 || bytes_per_sector != block_io.block_size() {
            return Err(FsError::BlockSizeMismatch);
        }

        let sectors_per_cluster = boot[0x0D];
        if sectors_per_cluster == 0 {
            return Err(FsError::InvalidSignature);
        }

        let total_sectors = read_u64(&boot, 0x28)?;
        let mft_lcn = read_u64(&boot, 0x30)?;
        let cluster_size = u64::from(bytes_per_sector)
            .checked_mul(u64::from(sectors_per_cluster))
            .ok_or(FsError::Corrupted)?;
        let file_record_size = decode_ntfs_size(boot[0x40] as i8, cluster_size)?;
        let index_record_size = decode_ntfs_size(boot[0x44] as i8, cluster_size)?;

        let mut fs = Self {
            block_io,
            bytes_per_sector,
            sectors_per_cluster,
            cluster_size,
            total_sectors,
            mft_lcn,
            file_record_size,
            index_record_size,
            mft_runs: Vec::new(),
        };

        let mft_record = fs.read_boot_mft_record()?;
        let mft_record = fs.parse_file_record(0, mft_record)?;
        let mft_data = mft_record
            .unnamed_attribute(ATTR_TYPE_DATA)
            .ok_or(FsError::Corrupted)?;
        match &mft_data.data {
            AttributeData::NonResident { runs, .. } => fs.mft_runs = runs.clone(),
            AttributeData::Resident { .. } => return Err(FsError::UnsupportedFs),
        }
        if fs.mft_runs.is_empty() {
            return Err(FsError::Corrupted);
        }

        Ok(fs)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        let record = if path == "/" || path.is_empty() {
            MFT_RECORD_ROOT
        } else {
            self.path_to_record(path)?
        };

        self.read_directory(record)
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let info = self.stat(path)?;
        if info.is_dir {
            return Err(FsError::NotFile);
        }

        let record = self.read_file_record(info.start_cluster)?;
        let data_attributes = record.unnamed_attributes(ATTR_TYPE_DATA)?;
        if data_attributes.is_empty() {
            return Err(FsError::NotFile);
        }
        self.read_attributes(&data_attributes, offset, info.size, buf)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        if path == "/" || path.is_empty() {
            return Ok(FileInfo::new(String::from("/"), 0, true, MFT_RECORD_ROOT));
        }

        let (dir, name) = crate::split_path(path);
        let entries = self.read_dir(&dir)?;
        entries
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(&name))
            .ok_or(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.bytes_per_sector
    }

    fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        let info = self.stat(path)?;
        if info.is_dir {
            return Err(FsError::NotFile);
        }

        let record = self.read_file_record(info.start_cluster)?;
        let data_attributes = record.unnamed_attributes(ATTR_TYPE_DATA)?;
        if data_attributes.is_empty() {
            return Err(FsError::NotFile);
        }
        if data_attributes.len() == 1 {
            if matches!(&data_attributes[0].data, AttributeData::Resident { .. }) {
                return Ok(Vec::new());
            }
        }

        let runs = collect_nonresident_runs(&data_attributes)?;
        self.runs_to_extents(&runs, info.size)
    }
}

impl Ntfs {
    /// Open an NTFS filesystem from a shared block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    fn path_to_record(&self, path: &str) -> Result<u64, FsError> {
        let mut record = MFT_RECORD_ROOT;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let entries = self.read_directory(record)?;
            let entry = entries
                .into_iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(part))
                .ok_or(FsError::DirectoryNotFound)?;
            if !entry.is_dir {
                return Err(FsError::NotDirectory);
            }
            record = entry.start_cluster;
        }
        Ok(record)
    }

    fn read_directory(&self, record_number: u64) -> Result<Vec<FileInfo>, FsError> {
        let record = self.read_file_record(record_number)?;
        if !record.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let mut entries = Vec::new();
        if let Some(index_root) = record.attribute(ATTR_TYPE_INDEX_ROOT) {
            self.parse_index_root(index_root, &mut entries)?;
        }
        let index_allocations = record.attributes(ATTR_TYPE_INDEX_ALLOCATION)?;
        if !index_allocations.is_empty() {
            self.parse_index_allocations(&index_allocations, &mut entries)?;
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
        Ok(entries)
    }

    fn read_boot_mft_record(&self) -> Result<Vec<u8>, FsError> {
        let mut data = alloc_buffer(self.file_record_size as usize)?;
        let byte_offset = self
            .mft_lcn
            .checked_mul(self.cluster_size)
            .ok_or(FsError::Corrupted)?;
        self.read_physical_bytes(byte_offset, &mut data)?;
        Ok(data)
    }

    fn read_file_record(&self, record_number: u64) -> Result<FileRecord, FsError> {
        let mut record = self.read_file_record_base(record_number)?;
        self.expand_attribute_list(&mut record)?;
        Ok(record)
    }

    fn read_file_record_base(&self, record_number: u64) -> Result<FileRecord, FsError> {
        let mut data = alloc_buffer(self.file_record_size as usize)?;
        let offset = record_number
            .checked_mul(u64::from(self.file_record_size))
            .ok_or(FsError::Corrupted)?;
        self.read_from_runs(&self.mft_runs, offset, &mut data, false)?;
        self.parse_file_record(record_number, data)
    }

    fn parse_file_record(
        &self,
        record_number: u64,
        mut data: Vec<u8>,
    ) -> Result<FileRecord, FsError> {
        if data.len() < 0x30 || &data[0..4] != FILE_RECORD_MAGIC {
            return Err(FsError::Corrupted);
        }

        apply_update_sequence(&mut data, self.bytes_per_sector as usize)?;

        let attrs_offset = read_u16(&data, 0x14)? as usize;
        let flags = read_u16(&data, 0x16)?;
        if flags & FILE_FLAG_IN_USE == 0 {
            return Err(FsError::FileNotFound);
        }
        if attrs_offset >= data.len() {
            return Err(FsError::Corrupted);
        }

        let mut attributes = Vec::new();
        let mut offset = attrs_offset;
        while offset + 8 <= data.len() {
            let attr_type = read_u32(&data, offset)?;
            if attr_type == ATTR_TYPE_END {
                break;
            }

            let attr_len = read_u32(&data, offset + 4)? as usize;
            if attr_len == 0
                || offset
                    .checked_add(attr_len)
                    .map_or(true, |end| end > data.len())
            {
                return Err(FsError::Corrupted);
            }

            let attr_data = &data[offset..offset + attr_len];
            if let Some(attribute) = parse_attribute(attr_type, attr_data)? {
                attributes
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                attributes.push(attribute);
            }
            offset += attr_len;
        }

        Ok(FileRecord {
            record_number,
            flags,
            attributes,
        })
    }

    fn expand_attribute_list(&self, record: &mut FileRecord) -> Result<(), FsError> {
        let entries = self.attribute_list_entries(record)?;
        if entries.is_empty() {
            return Ok(());
        }

        let mut extension_records = Vec::new();
        for entry in &entries {
            if entry.record_number == record.record_number
                || entry.attr_type == ATTR_TYPE_ATTRIBUTE_LIST
                || extension_records.contains(&entry.record_number)
            {
                continue;
            }
            extension_records
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extension_records.push(entry.record_number);
        }

        for record_number in extension_records {
            let extension = self.read_file_record_base(record_number)?;
            for attribute in extension.attributes {
                if attribute.attr_type == ATTR_TYPE_ATTRIBUTE_LIST {
                    continue;
                }
                if !entries.iter().any(|entry| {
                    entry.record_number == record_number && entry.attr_type == attribute.attr_type
                }) {
                    continue;
                }
                record
                    .attributes
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                record.attributes.push(attribute);
            }
        }

        record.attributes.sort_by(|a, b| {
            a.attr_type
                .cmp(&b.attr_type)
                .then_with(|| a.name_len.cmp(&b.name_len))
                .then_with(|| a.lowest_vcn.cmp(&b.lowest_vcn))
        });
        Ok(())
    }

    fn attribute_list_entries(
        &self,
        record: &FileRecord,
    ) -> Result<Vec<AttributeListEntry>, FsError> {
        let mut entries = Vec::new();
        for attribute in record.attributes(ATTR_TYPE_ATTRIBUTE_LIST)? {
            let size = attribute.data_size()?;
            if size == 0 {
                continue;
            }
            let size = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
            let mut data = alloc_buffer(size)?;
            self.read_attribute(attribute, 0, size as u64, &mut data)?;
            parse_attribute_list_entries(&data, &mut entries)?;
        }
        Ok(entries)
    }

    fn parse_index_root(
        &self,
        attribute: &NtfsAttribute,
        entries: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let value = attribute.resident_value()?;
        if value.len() < 32 {
            return Err(FsError::Corrupted);
        }

        let index_header = 16usize;
        let entries_offset = read_u32(value, index_header)? as usize;
        let total_size = read_u32(value, index_header + 4)? as usize;
        let flags = *value.get(index_header + 12).ok_or(FsError::Corrupted)?;
        let start = index_header
            .checked_add(entries_offset)
            .ok_or(FsError::Corrupted)?;
        let end = index_header
            .checked_add(total_size)
            .ok_or(FsError::Corrupted)?
            .min(value.len());
        if start > end {
            return Err(FsError::Corrupted);
        }

        parse_index_entries(&value[start..end], entries)?;
        if flags & INDEX_HEADER_HAS_ALLOCATION != 0 {
            // The caller will parse $INDEX_ALLOCATION when it is present.
            return Ok(());
        }
        Ok(())
    }

    fn parse_index_allocations(
        &self,
        attributes: &[&NtfsAttribute],
        entries: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let runs = collect_nonresident_runs(attributes)?;
        let size = runs
            .iter()
            .filter_map(|run| {
                run.virtual_cluster_start
                    .checked_add(run.cluster_count)
                    .and_then(|cluster| cluster.checked_mul(self.cluster_size))
            })
            .max()
            .unwrap_or(0);
        if size == 0 {
            return Ok(());
        }

        let size = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
        let mut data = alloc_buffer(size)?;
        self.read_from_runs(&runs, 0, &mut data, true)?;

        let record_size = self.index_record_size as usize;
        if record_size == 0 {
            return Err(FsError::Corrupted);
        }

        for chunk in data.chunks_mut(record_size) {
            if chunk.len() < record_size || &chunk[0..4] != INDEX_RECORD_MAGIC {
                continue;
            }
            apply_update_sequence(chunk, self.bytes_per_sector as usize)?;
            if chunk.len() < 40 {
                return Err(FsError::Corrupted);
            }

            let index_header = 24usize;
            let entries_offset = read_u32(chunk, index_header)? as usize;
            let total_size = read_u32(chunk, index_header + 4)? as usize;
            let start = index_header
                .checked_add(entries_offset)
                .ok_or(FsError::Corrupted)?;
            let end = index_header
                .checked_add(total_size)
                .ok_or(FsError::Corrupted)?
                .min(chunk.len());
            if start > end {
                return Err(FsError::Corrupted);
            }
            parse_index_entries(&chunk[start..end], entries)?;
        }

        Ok(())
    }

    fn read_attribute(
        &self,
        attribute: &NtfsAttribute,
        offset: u64,
        file_size: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= file_size || buf.is_empty() {
            return Ok(0);
        }

        let to_read = buf
            .len()
            .min(usize::try_from(file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        match &attribute.data {
            AttributeData::Resident { value } => {
                let start = usize::try_from(offset).map_err(|_| FsError::FileTooLarge)?;
                let end = start.checked_add(to_read).ok_or(FsError::Corrupted)?;
                let source = value.get(start..end).ok_or(FsError::ReadError)?;
                buf[..to_read].copy_from_slice(source);
                Ok(to_read)
            }
            AttributeData::NonResident { runs, .. } => {
                self.read_from_runs(runs, offset, &mut buf[..to_read], true)?;
                Ok(to_read)
            }
        }
    }

    fn read_attributes(
        &self,
        attributes: &[&NtfsAttribute],
        offset: u64,
        file_size: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if attributes.is_empty() {
            return Err(FsError::NotFile);
        }
        if attributes.len() == 1 {
            return self.read_attribute(attributes[0], offset, file_size, buf);
        }
        if attributes
            .iter()
            .any(|attribute| matches!(&attribute.data, AttributeData::Resident { .. }))
        {
            return Err(FsError::UnsupportedFs);
        }
        if offset >= file_size || buf.is_empty() {
            return Ok(0);
        }

        let to_read = buf
            .len()
            .min(usize::try_from(file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        let runs = collect_nonresident_runs(attributes)?;
        self.read_from_runs(&runs, offset, &mut buf[..to_read], true)?;
        Ok(to_read)
    }

    fn read_from_runs(
        &self,
        runs: &[DataRun],
        offset: u64,
        buf: &mut [u8],
        zero_sparse: bool,
    ) -> Result<(), FsError> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(FsError::Corrupted)?;
        let mut cursor = offset;
        let mut copied = 0usize;

        while copied < buf.len() {
            let run = runs
                .iter()
                .find(|run| {
                    let start = run.virtual_cluster_start.saturating_mul(self.cluster_size);
                    let end =
                        start.saturating_add(run.cluster_count.saturating_mul(self.cluster_size));
                    cursor >= start && cursor < end
                })
                .ok_or(FsError::ReadError)?;
            let run_start = run
                .virtual_cluster_start
                .checked_mul(self.cluster_size)
                .ok_or(FsError::Corrupted)?;
            let run_bytes = run
                .cluster_count
                .checked_mul(self.cluster_size)
                .ok_or(FsError::Corrupted)?;
            let run_end = run_start.checked_add(run_bytes).ok_or(FsError::Corrupted)?;
            let read_end = end.min(run_end);
            let read_len = usize::try_from(read_end - cursor).map_err(|_| FsError::FileTooLarge)?;

            if let Some(logical_cluster_start) = run.logical_cluster_start {
                let physical_byte = logical_cluster_start
                    .checked_mul(self.cluster_size)
                    .and_then(|start| start.checked_add(cursor - run_start))
                    .ok_or(FsError::Corrupted)?;
                self.read_physical_bytes(physical_byte, &mut buf[copied..copied + read_len])?;
            } else if zero_sparse {
                buf[copied..copied + read_len].fill(0);
            } else {
                return Err(FsError::UnsupportedFs);
            }

            cursor = read_end;
            copied += read_len;
        }

        Ok(())
    }

    fn read_physical_bytes(&self, physical_byte: u64, buf: &mut [u8]) -> Result<(), FsError> {
        if buf.is_empty() {
            return Ok(());
        }

        let block_size = self.bytes_per_sector as u64;
        if block_size == 0 {
            return Err(FsError::InvalidArgument);
        }
        let end = physical_byte
            .checked_add(buf.len() as u64)
            .ok_or(FsError::ReadError)?;
        let disk_size = self
            .total_sectors
            .checked_mul(block_size)
            .ok_or(FsError::ReadError)?;
        if end > disk_size {
            return Err(FsError::ReadError);
        }

        let mut copied = 0usize;
        let mut cursor = physical_byte;
        let mut block = alloc_buffer(self.bytes_per_sector as usize)?;
        while copied < buf.len() {
            let lba = cursor / block_size;
            let in_block = (cursor % block_size) as usize;
            read_full_blocks(self.block_io.as_ref(), lba, &mut block)?;
            let available = block.len().saturating_sub(in_block);
            let to_copy = available.min(buf.len() - copied);
            buf[copied..copied + to_copy].copy_from_slice(&block[in_block..in_block + to_copy]);
            copied += to_copy;
            cursor = cursor
                .checked_add(to_copy as u64)
                .ok_or(FsError::ReadError)?;
        }

        Ok(())
    }

    fn runs_to_extents(
        &self,
        runs: &[DataRun],
        file_size: u64,
    ) -> Result<Vec<FileExtent>, FsError> {
        let mut extents = Vec::new();
        if file_size == 0 {
            return Ok(extents);
        }

        let blocks_per_cluster = u64::from(self.sectors_per_cluster);
        let mut remaining_blocks =
            div_round_up(file_size, u64::from(self.bytes_per_sector)).ok_or(FsError::Corrupted)?;

        for run in runs {
            let Some(logical_cluster_start) = run.logical_cluster_start else {
                return Err(FsError::UnsupportedFs);
            };
            let run_blocks = run
                .cluster_count
                .checked_mul(blocks_per_cluster)
                .ok_or(FsError::Corrupted)?;
            let block_count = run_blocks.min(remaining_blocks);
            if block_count == 0 {
                break;
            }

            push_extent(
                &mut extents,
                run.virtual_cluster_start
                    .checked_mul(blocks_per_cluster)
                    .ok_or(FsError::Corrupted)?,
                logical_cluster_start
                    .checked_mul(blocks_per_cluster)
                    .ok_or(FsError::Corrupted)?,
                block_count,
            );
            remaining_blocks -= block_count;
            if remaining_blocks == 0 {
                break;
            }
        }

        if remaining_blocks == 0 {
            Ok(extents)
        } else {
            Err(FsError::Corrupted)
        }
    }
}

#[derive(Clone)]
struct DataRun {
    virtual_cluster_start: u64,
    logical_cluster_start: Option<u64>,
    cluster_count: u64,
}

struct NtfsAttribute {
    attr_type: u32,
    name_len: u8,
    lowest_vcn: u64,
    data: AttributeData,
}

impl NtfsAttribute {
    fn resident_value(&self) -> Result<&[u8], FsError> {
        match &self.data {
            AttributeData::Resident { value } => Ok(value),
            AttributeData::NonResident { .. } => Err(FsError::UnsupportedFs),
        }
    }

    fn data_size(&self) -> Result<u64, FsError> {
        match &self.data {
            AttributeData::Resident { value } => {
                u64::try_from(value.len()).map_err(|_| FsError::FileTooLarge)
            }
            AttributeData::NonResident { real_size, .. } => Ok(*real_size),
        }
    }
}

enum AttributeData {
    Resident { value: Vec<u8> },
    NonResident { real_size: u64, runs: Vec<DataRun> },
}

struct AttributeListEntry {
    attr_type: u32,
    record_number: u64,
}

struct FileRecord {
    record_number: u64,
    flags: u16,
    attributes: Vec<NtfsAttribute>,
}

impl FileRecord {
    fn is_directory(&self) -> bool {
        self.flags & FILE_FLAG_DIRECTORY != 0
    }

    fn unnamed_attribute(&self, attr_type: u32) -> Option<&NtfsAttribute> {
        self.attributes
            .iter()
            .find(|attr| attr.attr_type == attr_type && attr.name_len == 0)
    }

    fn attribute(&self, attr_type: u32) -> Option<&NtfsAttribute> {
        self.attributes
            .iter()
            .find(|attr| attr.attr_type == attr_type)
    }

    fn attributes(&self, attr_type: u32) -> Result<Vec<&NtfsAttribute>, FsError> {
        let mut out = Vec::new();
        for attr in self
            .attributes
            .iter()
            .filter(|attr| attr.attr_type == attr_type)
        {
            out.try_reserve_exact(1).map_err(|_| FsError::OutOfMemory)?;
            out.push(attr);
        }
        Ok(out)
    }

    fn unnamed_attributes(&self, attr_type: u32) -> Result<Vec<&NtfsAttribute>, FsError> {
        let mut out = Vec::new();
        for attr in self
            .attributes
            .iter()
            .filter(|attr| attr.attr_type == attr_type && attr.name_len == 0)
        {
            out.try_reserve_exact(1).map_err(|_| FsError::OutOfMemory)?;
            out.push(attr);
        }
        Ok(out)
    }
}

fn collect_nonresident_runs(attributes: &[&NtfsAttribute]) -> Result<Vec<DataRun>, FsError> {
    let mut runs = Vec::new();
    for attribute in attributes {
        match &attribute.data {
            AttributeData::Resident { .. } => return Err(FsError::UnsupportedFs),
            AttributeData::NonResident {
                runs: attr_runs, ..
            } => {
                runs.try_reserve_exact(attr_runs.len())
                    .map_err(|_| FsError::OutOfMemory)?;
                runs.extend(attr_runs.iter().cloned());
            }
        }
    }

    runs.sort_by_key(|run| run.virtual_cluster_start);
    Ok(runs)
}

fn parse_attribute(attr_type: u32, data: &[u8]) -> Result<Option<NtfsAttribute>, FsError> {
    if data.len() < 16 {
        return Err(FsError::Corrupted);
    }

    let non_resident = data[8] != 0;
    let name_len = data[9];
    let attr_flags = read_u16(data, 12)?;
    if attr_flags & 0x4001 != 0 {
        return Err(FsError::UnsupportedFs);
    }

    if non_resident {
        if data.len() < 64 {
            return Err(FsError::Corrupted);
        }

        let lowest_vcn = read_u64(data, 16)?;
        let runlist_offset = read_u16(data, 32)? as usize;
        let real_size = read_u64(data, 48)?;
        if runlist_offset >= data.len() {
            return Err(FsError::Corrupted);
        }
        let runs = parse_data_runs(&data[runlist_offset..], lowest_vcn)?;
        Ok(Some(NtfsAttribute {
            attr_type,
            name_len,
            lowest_vcn,
            data: AttributeData::NonResident { real_size, runs },
        }))
    } else {
        if data.len() < 24 {
            return Err(FsError::Corrupted);
        }

        let value_len = read_u32(data, 16)? as usize;
        let value_offset = read_u16(data, 20)? as usize;
        let end = value_offset
            .checked_add(value_len)
            .ok_or(FsError::Corrupted)?;
        if end > data.len() {
            return Err(FsError::Corrupted);
        }

        let mut value = Vec::new();
        value
            .try_reserve_exact(value_len)
            .map_err(|_| FsError::OutOfMemory)?;
        value.extend_from_slice(&data[value_offset..end]);
        Ok(Some(NtfsAttribute {
            attr_type,
            name_len,
            lowest_vcn: 0,
            data: AttributeData::Resident { value },
        }))
    }
}

fn parse_attribute_list_entries(
    data: &[u8],
    out: &mut Vec<AttributeListEntry>,
) -> Result<(), FsError> {
    let mut offset = 0usize;
    while offset + 26 <= data.len() {
        let attr_type = read_u32(data, offset)?;
        let entry_len = read_u16(data, offset + 4)? as usize;
        if attr_type == 0 || entry_len == 0 {
            break;
        }
        if entry_len < 26
            || offset
                .checked_add(entry_len)
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        let record_number = read_file_reference(data, offset + 16)?;
        out.try_reserve_exact(1).map_err(|_| FsError::OutOfMemory)?;
        out.push(AttributeListEntry {
            attr_type,
            record_number,
        });

        offset += entry_len;
    }

    Ok(())
}

fn parse_data_runs(data: &[u8], lowest_vcn: u64) -> Result<Vec<DataRun>, FsError> {
    let mut runs = Vec::new();
    let mut offset = 0usize;
    let mut current_vcn = lowest_vcn;
    let mut current_lcn = 0i64;

    while offset < data.len() {
        let header = data[offset];
        offset += 1;
        if header == 0 {
            break;
        }

        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        if len_size == 0
            || len_size > 8
            || off_size > 8
            || offset
                .checked_add(len_size)
                .and_then(|value| value.checked_add(off_size))
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        let cluster_count = read_le_uint(&data[offset..offset + len_size]);
        offset += len_size;
        if cluster_count == 0 {
            return Err(FsError::Corrupted);
        }

        let logical_cluster_start = if off_size == 0 {
            None
        } else {
            let delta = read_le_int(&data[offset..offset + off_size]);
            offset += off_size;
            current_lcn = current_lcn.checked_add(delta).ok_or(FsError::Corrupted)?;
            if current_lcn < 0 {
                return Err(FsError::Corrupted);
            }
            Some(current_lcn as u64)
        };

        runs.try_reserve_exact(1)
            .map_err(|_| FsError::OutOfMemory)?;
        runs.push(DataRun {
            virtual_cluster_start: current_vcn,
            logical_cluster_start,
            cluster_count,
        });
        current_vcn = current_vcn
            .checked_add(cluster_count)
            .ok_or(FsError::Corrupted)?;
    }

    Ok(runs)
}

fn parse_index_entries(data: &[u8], entries: &mut Vec<FileInfo>) -> Result<(), FsError> {
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        let entry_len = read_u16(data, offset + 8)? as usize;
        let stream_len = read_u16(data, offset + 10)? as usize;
        let flags = read_u16(data, offset + 12)?;
        if entry_len == 0
            || offset
                .checked_add(entry_len)
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        if flags & INDEX_ENTRY_LAST != 0 {
            break;
        }
        let stream_start = offset + 16;
        let stream_end = stream_start
            .checked_add(stream_len)
            .ok_or(FsError::Corrupted)?;
        if stream_end > offset + entry_len {
            return Err(FsError::Corrupted);
        }

        if let Some(info) =
            parse_file_name_entry(read_u48(data, offset)?, &data[stream_start..stream_end])?
        {
            entries
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            entries.push(info);
        }

        if flags & INDEX_ENTRY_HAS_CHILD != 0 {
            // Child VCN is stored in the entry tail; we only need the flattened
            // entries present in root/index allocation records for read-only use.
        }
        offset += entry_len;
    }

    Ok(())
}

fn parse_file_name_entry(record_number: u64, data: &[u8]) -> Result<Option<FileInfo>, FsError> {
    if data.len() < 66 {
        return Ok(None);
    }

    let namespace = data[65];
    if namespace == FILE_NAME_NAMESPACE_DOS {
        return Ok(None);
    }

    let allocated_size = read_u64(data, 40)?;
    let real_size = read_u64(data, 48)?;
    let raw_flags = read_u32(data, 56)?;
    let name_len = data[64] as usize;
    let name_bytes = name_len.checked_mul(2).ok_or(FsError::Corrupted)?;
    let name_start = 66usize;
    let name_end = name_start
        .checked_add(name_bytes)
        .ok_or(FsError::Corrupted)?;
    if name_end > data.len() {
        return Err(FsError::Corrupted);
    }

    let name = utf16le_to_string(&data[name_start..name_end])?;
    if name.is_empty() || name == "." || name == ".." {
        return Ok(None);
    }

    let is_dir = raw_flags & FILE_ATTRIBUTE_DIRECTORY != 0;
    let mut attributes = FileAttributes::empty();
    if raw_flags & FILE_ATTRIBUTE_READ_ONLY != 0 {
        attributes |= FileAttributes::READ_ONLY;
    }
    if raw_flags & FILE_ATTRIBUTE_HIDDEN != 0 {
        attributes |= FileAttributes::HIDDEN;
    }
    if raw_flags & FILE_ATTRIBUTE_SYSTEM != 0 {
        attributes |= FileAttributes::SYSTEM;
    }
    if raw_flags & FILE_ATTRIBUTE_ARCHIVE != 0 {
        attributes |= FileAttributes::ARCHIVE;
    }
    if is_dir {
        attributes |= FileAttributes::DIRECTORY;
    }

    Ok(Some(FileInfo {
        name,
        size: if is_dir { allocated_size } else { real_size },
        is_dir,
        attributes,
        start_cluster: record_number,
        contiguous: false,
    }))
}

fn utf16le_to_string(data: &[u8]) -> Result<String, FsError> {
    if data.len() % 2 != 0 {
        return Err(FsError::Corrupted);
    }

    let mut out = String::new();
    for chunk in data.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        if value == 0 {
            break;
        }
        let Some(ch) = char::from_u32(value as u32) else {
            return Err(FsError::Corrupted);
        };
        out.try_reserve(ch.len_utf8())
            .map_err(|_| FsError::OutOfMemory)?;
        out.push(ch);
    }
    Ok(out)
}

fn apply_update_sequence(data: &mut [u8], sector_size: usize) -> Result<(), FsError> {
    if sector_size == 0 || data.len() < sector_size {
        return Err(FsError::Corrupted);
    }

    let usa_offset = read_u16(data, 4)? as usize;
    let usa_count = read_u16(data, 6)? as usize;
    if usa_count == 0
        || usa_offset
            .checked_add(usa_count.checked_mul(2).ok_or(FsError::Corrupted)?)
            .map_or(true, |end| end > data.len())
    {
        return Err(FsError::Corrupted);
    }

    let sequence = read_u16(data, usa_offset)?;
    let sector_count = data.len() / sector_size;
    if usa_count != sector_count + 1 {
        return Err(FsError::Corrupted);
    }

    for sector in 0..sector_count {
        let tail = (sector + 1)
            .checked_mul(sector_size)
            .and_then(|value| value.checked_sub(2))
            .ok_or(FsError::Corrupted)?;
        if read_u16(data, tail)? != sequence {
            return Err(FsError::Corrupted);
        }
        let replacement = read_u16(data, usa_offset + 2 * (sector + 1))?;
        data[tail..tail + 2].copy_from_slice(&replacement.to_le_bytes());
    }

    Ok(())
}

fn decode_ntfs_size(encoded: i8, cluster_size: u64) -> Result<u32, FsError> {
    if encoded > 0 {
        let size = cluster_size
            .checked_mul(encoded as u64)
            .ok_or(FsError::Corrupted)?;
        u32::try_from(size).map_err(|_| FsError::Corrupted)
    } else if encoded < 0 {
        let shift = encoded.unsigned_abs();
        if shift >= 32 {
            return Err(FsError::Corrupted);
        }
        Ok(1u32 << shift)
    } else {
        Err(FsError::Corrupted)
    }
}

fn push_extent(
    extents: &mut Vec<FileExtent>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    if block_count == 0 {
        return;
    }

    if let Some(last) = extents.last_mut() {
        if last.virtual_block_end() == virtual_block_start
            && last.physical_lba_end() == physical_lba
        {
            last.block_count += block_count;
            return;
        }
    }

    extents.push(FileExtent::new(
        virtual_block_start,
        physical_lba,
        block_count,
    ));
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::Corrupted)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::Corrupted)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 8).ok_or(FsError::Corrupted)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_u48(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 6).ok_or(FsError::Corrupted)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0, 0,
    ]))
}

fn read_file_reference(data: &[u8], offset: usize) -> Result<u64, FsError> {
    read_u48(data, offset)
}

fn read_le_uint(data: &[u8]) -> u64 {
    let mut value = 0u64;
    for (index, byte) in data.iter().enumerate() {
        value |= u64::from(*byte) << (index * 8);
    }
    value
}

fn read_le_int(data: &[u8]) -> i64 {
    if data.is_empty() {
        return 0;
    }

    let mut value = read_le_uint(data) as i64;
    let bits = data.len() * 8;
    if bits < 64 && data[data.len() - 1] & 0x80 != 0 {
        value |= (!0i64) << bits;
    }
    value
}

fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }
    value.checked_add(divisor - 1).map(|value| value / divisor)
}

/// Check whether a sector buffer looks like an NTFS boot sector.
pub fn is_ntfs(data: &[u8]) -> bool {
    data.len() >= 512 && &data[3..11] == NTFS_OEM_ID && data[510] == 0x55 && data[511] == 0xAA
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use alloc::vec;

    struct MemoryBlockIo {
        block_size: u32,
        data: Vec<u8>,
    }

    impl MemoryBlockIo {
        fn new(block_size: u32, blocks: usize) -> Self {
            Self {
                block_size,
                data: vec![0; block_size as usize * blocks],
            }
        }

        fn block_mut(&mut self, lba: usize) -> &mut [u8] {
            let block_size = self.block_size as usize;
            let start = lba * block_size;
            &mut self.data[start..start + block_size]
        }

        fn bytes_mut(&mut self, offset: usize, len: usize) -> &mut [u8] {
            &mut self.data[offset..offset + len]
        }
    }

    impl crate::BlockIoOps for MemoryBlockIo {
        fn block_size(&self) -> u32 {
            self.block_size
        }

        fn total_blocks(&self) -> u64 {
            (self.data.len() / self.block_size as usize) as u64
        }

        fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            let block_size = self.block_size as usize;
            let start = lba as usize * block_size;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(FsError::ReadError);
            }
            buf.copy_from_slice(&self.data[start..end]);
            Ok(())
        }
    }

    #[test]
    fn reads_file_and_extents_from_minimal_ntfs() {
        let mut disk = MemoryBlockIo::new(512, 80);
        write_boot_sector(&mut disk);
        write_test_file_data(&mut disk);
        write_mft_record(
            &mut disk,
            0,
            false,
            &[data_attr_nonresident(16, &[(4, 16)])],
        );
        write_mft_record(
            &mut disk,
            5,
            true,
            &[index_root_attr(&[index_entry(
                6,
                "TEST.ISO",
                600,
                FILE_ATTRIBUTE_ARCHIVE,
            )])],
        );
        write_mft_record(
            &mut disk,
            6,
            false,
            &[data_attr_nonresident(600, &[(40, 2)])],
        );

        let fs = Ntfs::open(Rc::new(disk)).expect("open ntfs");
        let entries = fs.read_dir("/").expect("read root");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "TEST.ISO");
        assert_eq!(entries[0].size, 600);

        let info = fs.stat("/test.iso").expect("stat file");
        assert_eq!(info.start_cluster, 6);

        let extents = fs.file_extents("/TEST.ISO").expect("file extents");
        assert_eq!(extents, [FileExtent::new(0, 40, 2)]);

        let mut data = vec![0; 600];
        let read = fs.read_file("/TEST.ISO", 0, &mut data).expect("read file");
        assert_eq!(read, 600);
        assert_eq!(&data[..11], b"hello ntfs!");
        assert_eq!(data[599], 0x5A);
    }

    #[test]
    fn detects_ntfs_boot_sector() {
        let mut disk = MemoryBlockIo::new(512, 8);
        write_boot_sector(&mut disk);
        assert_eq!(
            crate::detect_fs_type(disk.block_mut(0)),
            FileSystemType::Ntfs
        );
        assert!(is_ntfs(disk.block_mut(0)));
    }

    #[test]
    fn follows_attribute_list_for_split_data_runs() {
        let mut disk = MemoryBlockIo::new(512, 96);
        write_boot_sector(&mut disk);
        disk.bytes_mut(50 * 512, 512)[..9].copy_from_slice(b"split-one");
        disk.bytes_mut(60 * 512, 512)[..9].copy_from_slice(b"split-two");
        disk.bytes_mut(60 * 512 + 387, 1)[0] = 0x7E;

        write_mft_record(
            &mut disk,
            0,
            false,
            &[data_attr_nonresident(24 * 512, &[(4, 24)])],
        );
        write_mft_record(
            &mut disk,
            5,
            true,
            &[index_root_attr(&[index_entry(
                7,
                "SPLIT.ISO",
                900,
                FILE_ATTRIBUTE_ARCHIVE,
            )])],
        );
        write_mft_record(
            &mut disk,
            7,
            false,
            &[
                attribute_list_attr(&[(ATTR_TYPE_DATA, 0, 7), (ATTR_TYPE_DATA, 1, 8)]),
                data_attr_nonresident_with_vcn(900, 0, &[(50, 1)]),
            ],
        );
        write_mft_record(
            &mut disk,
            8,
            false,
            &[data_attr_nonresident_with_vcn(900, 1, &[(60, 1)])],
        );

        let fs = Ntfs::open(Rc::new(disk)).expect("open ntfs");
        let extents = fs.file_extents("/split.iso").expect("file extents");
        assert_eq!(
            extents,
            [FileExtent::new(0, 50, 1), FileExtent::new(1, 60, 1)]
        );

        let mut data = vec![0; 900];
        let read = fs.read_file("/SPLIT.ISO", 0, &mut data).expect("read file");
        assert_eq!(read, 900);
        assert_eq!(&data[..9], b"split-one");
        assert_eq!(&data[512..521], b"split-two");
        assert_eq!(data[899], 0x7E);
    }

    fn write_boot_sector(disk: &mut MemoryBlockIo) {
        let total_blocks = (disk.data.len() / disk.block_size as usize) as u64;
        let boot = disk.block_mut(0);
        boot[0] = 0xEB;
        boot[1] = 0x52;
        boot[2] = 0x90;
        boot[3..11].copy_from_slice(NTFS_OEM_ID);
        boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        boot[0x0D] = 1;
        boot[0x28..0x30].copy_from_slice(&total_blocks.to_le_bytes());
        boot[0x30..0x38].copy_from_slice(&4u64.to_le_bytes());
        boot[0x38..0x40].copy_from_slice(&8u64.to_le_bytes());
        boot[0x40] = (-10i8) as u8;
        boot[0x44] = (-10i8) as u8;
        boot[510] = 0x55;
        boot[511] = 0xAA;
    }

    fn write_test_file_data(disk: &mut MemoryBlockIo) {
        let data = disk.bytes_mut(40 * 512, 1024);
        data[..11].copy_from_slice(b"hello ntfs!");
        data[599] = 0x5A;
    }

    fn write_mft_record(disk: &mut MemoryBlockIo, record: usize, is_dir: bool, attrs: &[Vec<u8>]) {
        let offset = 4 * 512 + record * 1024;
        let rec = disk.bytes_mut(offset, 1024);
        rec.fill(0);
        rec[0..4].copy_from_slice(FILE_RECORD_MAGIC);
        rec[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        rec[6..8].copy_from_slice(&3u16.to_le_bytes());
        rec[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
        rec[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
        rec[0x16..0x18].copy_from_slice(&(if is_dir { 3u16 } else { 1u16 }).to_le_bytes());

        let mut cursor = 0x38usize;
        for attr in attrs {
            rec[cursor..cursor + attr.len()].copy_from_slice(attr);
            cursor += attr.len();
        }
        rec[cursor..cursor + 4].copy_from_slice(&ATTR_TYPE_END.to_le_bytes());
        cursor += 4;
        rec[0x18..0x1C].copy_from_slice(&(cursor as u32).to_le_bytes());
        rec[0x1C..0x20].copy_from_slice(&1024u32.to_le_bytes());

        apply_test_fixup(rec);
    }

    fn apply_test_fixup(record: &mut [u8]) {
        let sequence = 0xA55Au16;
        let tail0 = u16::from_le_bytes([record[510], record[511]]);
        let tail1 = u16::from_le_bytes([record[1022], record[1023]]);
        record[0x30..0x32].copy_from_slice(&sequence.to_le_bytes());
        record[0x32..0x34].copy_from_slice(&tail0.to_le_bytes());
        record[0x34..0x36].copy_from_slice(&tail1.to_le_bytes());
        record[510..512].copy_from_slice(&sequence.to_le_bytes());
        record[1022..1024].copy_from_slice(&sequence.to_le_bytes());
    }

    fn data_attr_nonresident(real_size: u64, runs: &[(i64, u64)]) -> Vec<u8> {
        data_attr_nonresident_with_vcn(real_size, 0, runs)
    }

    fn data_attr_nonresident_with_vcn(
        real_size: u64,
        lowest_vcn: u64,
        runs: &[(i64, u64)],
    ) -> Vec<u8> {
        let mut runlist = Vec::new();
        let mut previous_lcn = 0i64;
        let mut cluster_count = 0u64;
        for (lcn, len) in runs {
            let delta = *lcn - previous_lcn;
            previous_lcn = *lcn;
            cluster_count += *len;
            runlist.push(0x11);
            runlist.push(*len as u8);
            runlist.push(delta as u8);
        }
        runlist.push(0);

        let runlist_offset = 0x40usize;
        let attr_len = align_up(runlist_offset + runlist.len(), 8);
        let mut attr = vec![0; attr_len];
        attr[0..4].copy_from_slice(&ATTR_TYPE_DATA.to_le_bytes());
        attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
        attr[8] = 1;
        attr[0x10..0x18].copy_from_slice(&lowest_vcn.to_le_bytes());
        let highest_vcn = lowest_vcn + cluster_count.saturating_sub(1);
        attr[0x18..0x20].copy_from_slice(&highest_vcn.to_le_bytes());
        attr[0x20..0x22].copy_from_slice(&(runlist_offset as u16).to_le_bytes());
        attr[0x28..0x30].copy_from_slice(&real_size.to_le_bytes());
        attr[0x30..0x38].copy_from_slice(&real_size.to_le_bytes());
        attr[0x38..0x40].copy_from_slice(&real_size.to_le_bytes());
        attr[runlist_offset..runlist_offset + runlist.len()].copy_from_slice(&runlist);
        attr
    }

    fn attribute_list_attr(entries: &[(u32, u64, u64)]) -> Vec<u8> {
        let mut value = Vec::new();
        for (attr_type, lowest_vcn, record_number) in entries {
            let entry_len = 32usize;
            let start = value.len();
            value.resize(start + entry_len, 0);
            value[start..start + 4].copy_from_slice(&attr_type.to_le_bytes());
            value[start + 4..start + 6].copy_from_slice(&(entry_len as u16).to_le_bytes());
            value[start + 8..start + 16].copy_from_slice(&lowest_vcn.to_le_bytes());
            value[start + 16..start + 22].copy_from_slice(&(record_number.to_le_bytes()[0..6]));
            value[start + 24..start + 26].copy_from_slice(&1u16.to_le_bytes());
        }

        resident_attr(ATTR_TYPE_ATTRIBUTE_LIST, &value)
    }

    fn resident_attr(attr_type: u32, value: &[u8]) -> Vec<u8> {
        let value_offset = 0x18usize;
        let attr_len = align_up(value_offset + value.len(), 8);
        let mut attr = vec![0; attr_len];
        attr[0..4].copy_from_slice(&attr_type.to_le_bytes());
        attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
        attr[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
        attr[0x14..0x16].copy_from_slice(&(value_offset as u16).to_le_bytes());
        attr[value_offset..value_offset + value.len()].copy_from_slice(value);
        attr
    }

    fn index_root_attr(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut value = vec![0; 32];
        value[0..4].copy_from_slice(&ATTR_TYPE_FILE_NAME.to_le_bytes());
        value[8..12].copy_from_slice(&1024u32.to_le_bytes());
        value[12] = 1;
        value[16..20].copy_from_slice(&16u32.to_le_bytes());

        let mut entries_len = 0usize;
        for entry in entries {
            entries_len += entry.len();
            value.extend_from_slice(entry);
        }
        let mut last = vec![0; 16];
        last[8..10].copy_from_slice(&16u16.to_le_bytes());
        last[12..14].copy_from_slice(&INDEX_ENTRY_LAST.to_le_bytes());
        entries_len += last.len();
        value.extend_from_slice(&last);

        let total = 16 + entries_len;
        value[20..24].copy_from_slice(&(total as u32).to_le_bytes());
        value[24..28].copy_from_slice(&(total as u32).to_le_bytes());

        let value_offset = 0x18usize;
        let attr_len = align_up(value_offset + value.len(), 8);
        let mut attr = vec![0; attr_len];
        attr[0..4].copy_from_slice(&ATTR_TYPE_INDEX_ROOT.to_le_bytes());
        attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
        attr[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
        attr[0x14..0x16].copy_from_slice(&(value_offset as u16).to_le_bytes());
        attr[value_offset..value_offset + value.len()].copy_from_slice(&value);
        attr
    }

    fn index_entry(record: u64, name: &str, size: u64, attrs: u32) -> Vec<u8> {
        let mut file_name = vec![0; 66 + name.encode_utf16().count() * 2];
        file_name[40..48].copy_from_slice(&align_up(size as usize, 512).to_le_bytes());
        file_name[48..56].copy_from_slice(&size.to_le_bytes());
        file_name[56..60].copy_from_slice(&attrs.to_le_bytes());
        file_name[64] = name.encode_utf16().count() as u8;
        file_name[65] = 1;
        for (index, ch) in name.encode_utf16().enumerate() {
            let offset = 66 + index * 2;
            file_name[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }

        let entry_len = align_up(16 + file_name.len(), 8);
        let mut entry = vec![0; entry_len];
        entry[0..6].copy_from_slice(&(record as u64).to_le_bytes()[0..6].as_ref());
        entry[8..10].copy_from_slice(&(entry_len as u16).to_le_bytes());
        entry[10..12].copy_from_slice(&(file_name.len() as u16).to_le_bytes());
        entry[16..16 + file_name.len()].copy_from_slice(&file_name);
        entry
    }

    fn align_up(value: usize, align: usize) -> usize {
        (value + align - 1) / align * align
    }
}
