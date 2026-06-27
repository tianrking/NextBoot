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

mod indexes;
mod methods;
mod parser;

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
mod tests;
