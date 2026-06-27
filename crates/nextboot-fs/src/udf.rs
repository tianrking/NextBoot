//! Minimal read-only UDF filesystem support.
//!
//! This implements the subset needed by UEFI boot paths on hybrid Windows
//! installation media: type-1 partition maps, FE/EFE file entries, short/long
//! allocation descriptors, in-ICB data, and File Identifier Descriptor
//! directories.

use crate::{
    alloc_buffer, FileAttributes, FileExtent, FileInfo, FileSystem, FileSystemType, FsError,
    SharedBlockIo,
};
use alloc::string::String;
use alloc::vec::Vec;

mod mount;
mod nodes;
mod storage;

const TAG_IDENT_AVDP: u16 = 0x0002;
const TAG_IDENT_PD: u16 = 0x0005;
const TAG_IDENT_LVD: u16 = 0x0006;
const TAG_IDENT_TD: u16 = 0x0008;
const TAG_IDENT_FSD: u16 = 0x0100;
const TAG_IDENT_FID: u16 = 0x0101;
const TAG_IDENT_FE: u16 = 0x0105;
const TAG_IDENT_EFE: u16 = 0x010a;

const ICB_FILE_TYPE_DIRECTORY: u8 = 0x04;
const ICB_FILE_TYPE_REGULAR: u8 = 0x05;

const ICB_AD_SHORT: u16 = 0x0000;
const ICB_AD_LONG: u16 = 0x0001;
const ICB_AD_EXTENDED: u16 = 0x0002;
const ICB_AD_IN_ICB: u16 = 0x0003;
const ICB_AD_MASK: u16 = 0x0007;

const EXTENT_TYPE_MASK: u32 = 0xc000_0000;
const EXTENT_LENGTH_MASK: u32 = 0x3fff_ffff;

const FID_CHAR_HIDDEN: u8 = 0x01;
const FID_CHAR_DIRECTORY: u8 = 0x02;
const FID_CHAR_DELETED: u8 = 0x04;
const FID_CHAR_PARENT: u8 = 0x08;

const AVDP_CANDIDATES: &[u64] = &[256, 512];
const TAG_IDENT_OFFSET: usize = 0;
const TAG_LOCATION_OFFSET: usize = 12;
const AVDP_MAIN_VDS_LENGTH_OFFSET: usize = 16;
const AVDP_MAIN_VDS_START_OFFSET: usize = 20;
const PD_PARTITION_NUMBER_OFFSET: usize = 22;
const PD_PARTITION_START_OFFSET: usize = 188;
const PD_PARTITION_LENGTH_OFFSET: usize = 192;
const LVD_BLOCK_SIZE_OFFSET: usize = 212;
const LVD_ROOT_FILESET_OFFSET: usize = 248;
const LVD_MAP_TABLE_LENGTH_OFFSET: usize = 264;
const LVD_NUM_PARTITION_MAPS_OFFSET: usize = 268;
const LVD_PARTITION_MAPS_OFFSET: usize = 440;
const FSD_ROOT_ICB_OFFSET: usize = 400;
const FILE_ENTRY_ICB_FILE_TYPE_OFFSET: usize = 27;
const FILE_ENTRY_ICB_FLAGS_OFFSET: usize = 34;
const FILE_ENTRY_FILE_SIZE_OFFSET: usize = 56;
const FE_BLOCKS_RECORDED_OFFSET: usize = 64;
const FE_EXT_ATTR_LENGTH_OFFSET: usize = 168;
const FE_ALLOC_DESCS_LENGTH_OFFSET: usize = 172;
const FE_ALLOC_DESCS_OFFSET: usize = 176;
const EFE_OBJECT_SIZE_OFFSET: usize = 64;
const EFE_BLOCKS_RECORDED_OFFSET: usize = 72;
const EFE_EXT_ATTR_LENGTH_OFFSET: usize = 208;
const EFE_ALLOC_DESCS_LENGTH_OFFSET: usize = 212;
const EFE_ALLOC_DESCS_OFFSET: usize = 216;
const FID_HEADER_SIZE: usize = 38;
const FID_CHARACTERISTICS_OFFSET: usize = 18;
const FID_NAME_LENGTH_OFFSET: usize = 19;
const FID_ICB_OFFSET: usize = 20;
const FID_IMP_USE_LENGTH_OFFSET: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExtentAd {
    length: u32,
    start: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogicalBlockAddress {
    block_num: u32,
    part_ref: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LongAd {
    length: u32,
    block: LogicalBlockAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Partition {
    number: u16,
    start: u32,
    length: u32,
    descriptor_lba: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartitionMap {
    partition_index: usize,
}

#[derive(Debug, Clone)]
struct UdfNode {
    entry_lba: u64,
    tag_ident: u16,
    part_ref: u16,
    file_type: u8,
    flags: u16,
    file_size: u64,
    alloc_desc_offset: usize,
    alloc_desc_len: usize,
    entry: Vec<u8>,
}

impl UdfNode {
    fn is_dir(&self) -> bool {
        self.file_type == ICB_FILE_TYPE_DIRECTORY
    }

    fn is_file(&self) -> bool {
        self.file_type == ICB_FILE_TYPE_REGULAR
    }
}

/// A block-level patch that redirects a UDF file entry to replacement data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdfFileReplacementPatch {
    pub file_entry_offset: u64,
    pub file_entry_data: Vec<u8>,
    pub partition_descriptor: Option<UdfPartitionDescriptorPatch>,
}

/// A patched UDF partition descriptor block, needed when appended replacement
/// data extends past the original partition extent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdfPartitionDescriptorPatch {
    pub descriptor_offset: u64,
    pub descriptor_data: Vec<u8>,
}

/// Read-only UDF filesystem.
pub struct Udf {
    block_io: SharedBlockIo,
    block_size: u32,
    logical_block_size: u32,
    partitions: Vec<Partition>,
    partition_maps: Vec<PartitionMap>,
    root_icb: LongAd,
}

impl FileSystem for Udf {
    const FS_TYPE: FileSystemType = FileSystemType::Udf;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let block_size = block_io.block_size();
        if block_size == 0 {
            return Err(FsError::InvalidArgument);
        }

        let mut fs = Self {
            block_io,
            block_size,
            logical_block_size: block_size,
            partitions: Vec::new(),
            partition_maps: Vec::new(),
            root_icb: LongAd {
                length: 0,
                block: LogicalBlockAddress {
                    block_num: 0,
                    part_ref: 0,
                },
            },
        };
        fs.mount()?;
        Ok(fs)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        let node = self.find_node(path)?;
        if !node.is_dir() {
            return Err(FsError::NotDirectory);
        }

        self.read_dir_node(&node)
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }

        self.read_node_data(&node, offset, buf)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        if path == "/" || path.is_empty() {
            return Ok(FileInfo::new(String::from("/"), 0, true, 0));
        }

        let (dir, name) = crate::split_path(path);
        let entries = self.read_dir(&dir)?;
        entries
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(&name))
            .ok_or(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.logical_block_size
    }

    fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }

        self.node_extents(&node)
    }
}

impl Udf {
    /// Open a UDF filesystem from a shared block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UdfDirEntry {
    name: String,
    icb: LongAd,
    is_dir: bool,
    hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NodeExtent {
    length: u32,
    physical_lba: u64,
    extent_type: u32,
}

fn read_long_ad(data: &[u8], offset: usize) -> Result<LongAd, FsError> {
    Ok(LongAd {
        length: read_u32(data, offset)?,
        block: LogicalBlockAddress {
            block_num: read_u32(data, offset + 4)?,
            part_ref: read_u16(data, offset + 8)?,
        },
    })
}

fn decode_osta_name(raw: &[u8]) -> Result<String, FsError> {
    let Some((&compression, data)) = raw.split_first() else {
        return Err(FsError::Corrupted);
    };

    let mut out = String::new();
    match compression {
        8 => {
            out.try_reserve(data.len())
                .map_err(|_| FsError::OutOfMemory)?;
            for &byte in data {
                out.push(byte as char);
            }
        }
        16 => {
            if data.len() % 2 != 0 {
                return Err(FsError::Corrupted);
            }
            out.try_reserve(data.len() / 2)
                .map_err(|_| FsError::OutOfMemory)?;
            for unit in data.chunks_exact(2) {
                let ch = u16::from_be_bytes([unit[0], unit[1]]);
                out.push(char::from_u32(u32::from(ch)).unwrap_or('\u{fffd}'));
            }
        }
        _ => return Err(FsError::UnsupportedFs),
    }

    Ok(out)
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

fn write_u16(data: &mut [u8], offset: usize, value: u16) -> Result<(), FsError> {
    let bytes = data.get_mut(offset..offset + 2).ok_or(FsError::Corrupted)?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) -> Result<(), FsError> {
    let bytes = data.get_mut(offset..offset + 4).ok_or(FsError::Corrupted)?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(data: &mut [u8], offset: usize, value: u64) -> Result<(), FsError> {
    let bytes = data.get_mut(offset..offset + 8).ok_or(FsError::Corrupted)?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn refresh_descriptor_tag(block: &mut [u8]) -> Result<(), FsError> {
    if block.len() < 16 {
        return Err(FsError::Corrupted);
    }

    let crc_len = read_u16(block, 10)? as usize;
    if crc_len > 0 {
        let crc_end = 16usize.checked_add(crc_len).ok_or(FsError::Corrupted)?;
        if crc_end > block.len() {
            return Err(FsError::Corrupted);
        }
        let crc = udf_crc16(&block[16..crc_end]);
        write_u16(block, 8, crc)?;
    }

    block[4] = 0;
    let checksum = block[..16]
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 4)
        .fold(0u8, |sum, (_, byte)| sum.wrapping_add(*byte));
    block[4] = checksum;
    Ok(())
}

fn udf_crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn div_round_up(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return 0;
    }
    value.saturating_add(divisor - 1) / divisor
}
