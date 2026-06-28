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
mod util;
pub use util::is_ntfs;
use util::*;

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

#[cfg(test)]
mod tests;
