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

    fn mount(&mut self) -> Result<(), FsError> {
        let anchor = self.read_anchor()?;
        self.read_volume_descriptor_sequence(anchor)?;

        if self.partition_maps.is_empty() || self.partitions.is_empty() {
            return Err(FsError::InvalidSignature);
        }

        let fileset_lba = self.map_logical_block(self.root_icb.block)?;
        let fileset = self.read_logical_block(fileset_lba)?;
        if read_u16(&fileset, TAG_IDENT_OFFSET)? != TAG_IDENT_FSD {
            return Err(FsError::InvalidSignature);
        }

        self.root_icb = read_long_ad(&fileset, FSD_ROOT_ICB_OFFSET)?;
        Ok(())
    }

    fn read_anchor(&self) -> Result<ExtentAd, FsError> {
        for &lba in AVDP_CANDIDATES {
            if lba >= self.block_io.total_blocks() {
                continue;
            }

            let block = self.read_logical_block(lba)?;
            if read_u16(&block, TAG_IDENT_OFFSET)? == TAG_IDENT_AVDP
                && read_u32(&block, TAG_LOCATION_OFFSET)? == lba as u32
            {
                return Ok(ExtentAd {
                    length: read_u32(&block, AVDP_MAIN_VDS_LENGTH_OFFSET)?,
                    start: read_u32(&block, AVDP_MAIN_VDS_START_OFFSET)?,
                });
            }
        }

        Err(FsError::InvalidSignature)
    }

    fn read_volume_descriptor_sequence(&mut self, anchor: ExtentAd) -> Result<(), FsError> {
        let descriptor_count = ((u64::from(anchor.length) + u64::from(self.block_size) - 1)
            / u64::from(self.block_size))
        .max(1);
        let end = u64::from(anchor.start)
            .checked_add(descriptor_count)
            .ok_or(FsError::Corrupted)?;
        let mut block_lba = u64::from(anchor.start);

        while block_lba < end {
            let block = self.read_logical_block(block_lba)?;
            match read_u16(&block, TAG_IDENT_OFFSET)? {
                TAG_IDENT_PD => self.read_partition_descriptor(&block, block_lba)?,
                TAG_IDENT_LVD => self.read_logical_volume_descriptor(&block)?,
                TAG_IDENT_TD => break,
                ident if ident > TAG_IDENT_TD => return Err(FsError::InvalidSignature),
                _ => {}
            }
            block_lba += 1;
        }

        self.resolve_partition_maps()
    }

    fn read_partition_descriptor(
        &mut self,
        block: &[u8],
        descriptor_lba: u64,
    ) -> Result<(), FsError> {
        let partition = Partition {
            number: read_u16(block, PD_PARTITION_NUMBER_OFFSET)?,
            start: read_u32(block, PD_PARTITION_START_OFFSET)?,
            length: read_u32(block, PD_PARTITION_LENGTH_OFFSET)?,
            descriptor_lba,
        };
        self.partitions
            .try_reserve_exact(1)
            .map_err(|_| FsError::OutOfMemory)?;
        self.partitions.push(partition);
        Ok(())
    }

    fn read_logical_volume_descriptor(&mut self, block: &[u8]) -> Result<(), FsError> {
        let logical_block_size = read_u32(block, LVD_BLOCK_SIZE_OFFSET)?;
        if logical_block_size == 0 || logical_block_size != self.block_size {
            return Err(FsError::BlockSizeMismatch);
        }
        self.logical_block_size = logical_block_size;
        self.root_icb = read_long_ad(block, LVD_ROOT_FILESET_OFFSET)?;

        let map_table_len = read_u32(block, LVD_MAP_TABLE_LENGTH_OFFSET)? as usize;
        let map_count = read_u32(block, LVD_NUM_PARTITION_MAPS_OFFSET)? as usize;
        let maps_end = LVD_PARTITION_MAPS_OFFSET
            .checked_add(map_table_len)
            .ok_or(FsError::Corrupted)?;
        if maps_end > block.len() {
            return Err(FsError::Corrupted);
        }

        self.partition_maps.clear();
        self.partition_maps
            .try_reserve_exact(map_count)
            .map_err(|_| FsError::OutOfMemory)?;

        let mut offset = LVD_PARTITION_MAPS_OFFSET;
        for _ in 0..map_count {
            if offset + 6 > maps_end {
                return Err(FsError::Corrupted);
            }

            let map_type = block[offset];
            let map_len = block[offset + 1] as usize;
            if map_type != 1 || map_len < 6 || offset + map_len > maps_end {
                return Err(FsError::UnsupportedFs);
            }

            let partition_number = read_u16(block, offset + 4)?;
            self.partition_maps.push(PartitionMap {
                partition_index: partition_number as usize,
            });
            offset += map_len;
        }

        Ok(())
    }

    fn resolve_partition_maps(&mut self) -> Result<(), FsError> {
        for map in &mut self.partition_maps {
            let partition_number = map.partition_index as u16;
            let Some(index) = self
                .partitions
                .iter()
                .position(|partition| partition.number == partition_number)
            else {
                return Err(FsError::Corrupted);
            };
            map.partition_index = index;
        }
        Ok(())
    }

    fn find_node(&self, path: &str) -> Result<UdfNode, FsError> {
        let mut node = self.read_icb(self.root_icb)?;
        let parts = path.split('/').filter(|part| !part.is_empty());

        for part in parts {
            if !node.is_dir() {
                return Err(FsError::NotDirectory);
            }

            let mut found = None;
            for entry in self.read_dir_entries(&node)? {
                if entry.name.eq_ignore_ascii_case(part) {
                    found = Some(entry.icb);
                    break;
                }
            }

            let Some(icb) = found else {
                return Err(FsError::FileNotFound);
            };
            node = self.read_icb(icb)?;
        }

        Ok(node)
    }

    /// Build a replacement patch for `path` so that its file entry points at
    /// `replacement_lba` with `replacement_size` visible bytes.
    pub fn file_replacement_patch(
        &self,
        path: &str,
        replacement_lba: u64,
        replacement_size: u64,
        allocated_bytes: u64,
    ) -> Result<UdfFileReplacementPatch, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }

        let descriptor_size = match node.flags & ICB_AD_MASK {
            ICB_AD_SHORT | ICB_AD_IN_ICB => 8usize,
            ICB_AD_LONG => 16usize,
            ICB_AD_EXTENDED => return Err(FsError::UnsupportedFs),
            _ => return Err(FsError::UnsupportedFs),
        };

        if replacement_size > u64::from(EXTENT_LENGTH_MASK) {
            return Err(FsError::FileTooLarge);
        }

        let map = self
            .partition_maps
            .get(node.part_ref as usize)
            .ok_or(FsError::Corrupted)?;
        let partition = *self
            .partitions
            .get(map.partition_index)
            .ok_or(FsError::Corrupted)?;
        let replacement_block = replacement_lba
            .checked_sub(u64::from(partition.start))
            .ok_or(FsError::InvalidArgument)?;
        let replacement_block_u32 =
            u32::try_from(replacement_block).map_err(|_| FsError::FileTooLarge)?;
        let replacement_size_u32 =
            u32::try_from(replacement_size).map_err(|_| FsError::FileTooLarge)?;

        let mut entry = node.entry.clone();
        write_u64(&mut entry, FILE_ENTRY_FILE_SIZE_OFFSET, replacement_size)?;
        if node.tag_ident == TAG_IDENT_EFE {
            write_u64(&mut entry, EFE_OBJECT_SIZE_OFFSET, replacement_size)?;
            write_u64(&mut entry, EFE_BLOCKS_RECORDED_OFFSET, allocated_bytes)?;
        } else {
            write_u64(&mut entry, FE_BLOCKS_RECORDED_OFFSET, allocated_bytes)?;
        }
        let alloc_len_offset = if node.tag_ident == TAG_IDENT_FE {
            FE_ALLOC_DESCS_LENGTH_OFFSET
        } else {
            EFE_ALLOC_DESCS_LENGTH_OFFSET
        };
        write_u32(&mut entry, alloc_len_offset, descriptor_size as u32)?;

        let flags = (node.flags & !ICB_AD_MASK)
            | if descriptor_size == 16 {
                ICB_AD_LONG
            } else {
                ICB_AD_SHORT
            };
        write_u16(&mut entry, FILE_ENTRY_ICB_FLAGS_OFFSET, flags)?;

        let clear_len = node.alloc_desc_len.max(descriptor_size);
        let clear_end = node
            .alloc_desc_offset
            .checked_add(clear_len)
            .ok_or(FsError::Corrupted)?;
        if clear_end > entry.len() {
            return Err(FsError::Corrupted);
        }
        entry[node.alloc_desc_offset..clear_end].fill(0);
        write_u32(&mut entry, node.alloc_desc_offset, replacement_size_u32)?;
        write_u32(
            &mut entry,
            node.alloc_desc_offset + 4,
            replacement_block_u32,
        )?;
        if descriptor_size == 16 {
            write_u16(&mut entry, node.alloc_desc_offset + 8, node.part_ref)?;
        }
        refresh_descriptor_tag(&mut entry)?;

        let allocated_blocks = div_round_up(allocated_bytes, u64::from(self.logical_block_size));
        let replacement_end_lba = replacement_lba
            .checked_add(allocated_blocks)
            .ok_or(FsError::Corrupted)?;
        let partition_descriptor =
            self.partition_descriptor_patch(partition, replacement_end_lba)?;

        Ok(UdfFileReplacementPatch {
            file_entry_offset: node
                .entry_lba
                .checked_mul(u64::from(self.logical_block_size))
                .ok_or(FsError::Corrupted)?,
            file_entry_data: entry,
            partition_descriptor,
        })
    }

    fn read_dir_node(&self, node: &UdfNode) -> Result<Vec<FileInfo>, FsError> {
        let dir_entries = self.read_dir_entries(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(dir_entries.len())
            .map_err(|_| FsError::OutOfMemory)?;

        for entry in dir_entries {
            let child = self.read_icb(entry.icb)?;
            let mut info = FileInfo::new(
                entry.name,
                child.file_size,
                entry.is_dir || child.is_dir(),
                self.node_start_lba(&child).unwrap_or(0),
            );
            info.contiguous = self.node_is_contiguous(&child);
            if info.is_dir {
                info.attributes |= FileAttributes::DIRECTORY;
            }
            if entry.hidden {
                info.attributes |= FileAttributes::HIDDEN;
            }
            out.push(info);
        }

        Ok(out)
    }

    fn read_dir_entries(&self, node: &UdfNode) -> Result<Vec<UdfDirEntry>, FsError> {
        let size = usize::try_from(node.file_size).map_err(|_| FsError::FileTooLarge)?;
        let mut data = alloc_buffer(size)?;
        if size != 0 {
            self.read_node_data(node, 0, &mut data)?;
        }

        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < data.len() {
            if offset + FID_HEADER_SIZE > data.len() {
                break;
            }
            if read_u16(&data, offset + TAG_IDENT_OFFSET)? != TAG_IDENT_FID {
                return Err(FsError::Corrupted);
            }

            let characteristics = data[offset + FID_CHARACTERISTICS_OFFSET];
            let name_len = data[offset + FID_NAME_LENGTH_OFFSET] as usize;
            let icb = read_long_ad(&data, offset + FID_ICB_OFFSET)?;
            let imp_use_len = read_u16(&data, offset + FID_IMP_USE_LENGTH_OFFSET)? as usize;
            let name_offset = offset
                .checked_add(FID_HEADER_SIZE)
                .and_then(|value| value.checked_add(imp_use_len))
                .ok_or(FsError::Corrupted)?;
            let name_end = name_offset
                .checked_add(name_len)
                .ok_or(FsError::Corrupted)?;
            if name_end > data.len() {
                return Err(FsError::Corrupted);
            }

            if characteristics & (FID_CHAR_DELETED | FID_CHAR_PARENT) == 0 {
                let name = decode_osta_name(&data[name_offset..name_end])?;
                entries
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                entries.push(UdfDirEntry {
                    name,
                    icb,
                    is_dir: characteristics & FID_CHAR_DIRECTORY != 0,
                    hidden: characteristics & FID_CHAR_HIDDEN != 0,
                });
            }

            offset = align_up(name_end, 4).ok_or(FsError::Corrupted)?;
        }

        Ok(entries)
    }

    fn read_icb(&self, icb: LongAd) -> Result<UdfNode, FsError> {
        let lba = self.map_logical_block(icb.block)?;
        let entry = self.read_logical_block(lba)?;
        let tag_ident = read_u16(&entry, TAG_IDENT_OFFSET)?;
        if tag_ident != TAG_IDENT_FE && tag_ident != TAG_IDENT_EFE {
            return Err(FsError::Corrupted);
        }

        let (ext_attr_offset, alloc_len_offset, alloc_offset) = if tag_ident == TAG_IDENT_FE {
            (
                FE_EXT_ATTR_LENGTH_OFFSET,
                FE_ALLOC_DESCS_LENGTH_OFFSET,
                FE_ALLOC_DESCS_OFFSET,
            )
        } else {
            (
                EFE_EXT_ATTR_LENGTH_OFFSET,
                EFE_ALLOC_DESCS_LENGTH_OFFSET,
                EFE_ALLOC_DESCS_OFFSET,
            )
        };
        let ext_attr_len = read_u32(&entry, ext_attr_offset)? as usize;
        let alloc_desc_len = read_u32(&entry, alloc_len_offset)? as usize;
        let alloc_desc_offset = alloc_offset
            .checked_add(ext_attr_len)
            .ok_or(FsError::Corrupted)?;
        if alloc_desc_offset
            .checked_add(alloc_desc_len)
            .map_or(true, |end| end > entry.len())
        {
            return Err(FsError::Corrupted);
        }

        Ok(UdfNode {
            entry_lba: lba,
            tag_ident,
            part_ref: icb.block.part_ref,
            file_type: *entry
                .get(FILE_ENTRY_ICB_FILE_TYPE_OFFSET)
                .ok_or(FsError::Corrupted)?,
            flags: read_u16(&entry, FILE_ENTRY_ICB_FLAGS_OFFSET)?,
            file_size: read_u64(&entry, FILE_ENTRY_FILE_SIZE_OFFSET)?,
            alloc_desc_offset,
            alloc_desc_len,
            entry,
        })
    }

    fn read_node_data(
        &self,
        node: &UdfNode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= node.file_size || buf.is_empty() {
            return Ok(0);
        }

        let readable = buf
            .len()
            .min(usize::try_from(node.file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        if node.flags & ICB_AD_MASK == ICB_AD_IN_ICB {
            let start = node
                .alloc_desc_offset
                .checked_add(usize::try_from(offset).map_err(|_| FsError::FileTooLarge)?)
                .ok_or(FsError::Corrupted)?;
            let end = start.checked_add(readable).ok_or(FsError::Corrupted)?;
            if end > node.entry.len() {
                return Err(FsError::Corrupted);
            }
            buf[..readable].copy_from_slice(&node.entry[start..end]);
            return Ok(readable);
        }

        let extents = self.node_data_extents(node)?;
        let mut copied = 0usize;
        let mut file_cursor = 0u64;
        let block_size = u64::from(self.logical_block_size);

        for extent in extents {
            if extent.length == 0 || extent.extent_type != 0 {
                file_cursor = file_cursor.saturating_add(u64::from(extent.length));
                continue;
            }

            let extent_len = u64::from(extent.length);
            let extent_end = file_cursor.saturating_add(extent_len);
            if offset >= extent_end {
                file_cursor = extent_end;
                continue;
            }

            let within_extent = offset.saturating_sub(file_cursor);
            let lba_offset = within_extent / block_size;
            let in_block_offset = (within_extent % block_size) as usize;
            let mut current_lba = extent.physical_lba.saturating_add(lba_offset);
            let mut extent_remaining = (extent_len - within_extent) as usize;
            let mut block = alloc_buffer(self.logical_block_size as usize)?;

            while copied < readable && extent_remaining > 0 {
                self.read_full_logical_block(current_lba, &mut block)?;
                let source_offset = if lba_offset == 0 && copied == 0 {
                    in_block_offset
                } else {
                    0
                };
                let available = block
                    .len()
                    .saturating_sub(source_offset)
                    .min(extent_remaining);
                let to_copy = available.min(readable - copied);
                buf[copied..copied + to_copy]
                    .copy_from_slice(&block[source_offset..source_offset + to_copy]);
                copied += to_copy;
                extent_remaining -= to_copy;
                current_lba = current_lba.saturating_add(1);
            }

            if copied >= readable {
                break;
            }
            file_cursor = extent_end;
        }

        Ok(copied)
    }

    fn node_extents(&self, node: &UdfNode) -> Result<Vec<FileExtent>, FsError> {
        let data_extents = self.node_data_extents(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(data_extents.len())
            .map_err(|_| FsError::OutOfMemory)?;

        let mut virtual_block_start = 0u64;
        for extent in data_extents {
            if extent.extent_type != 0 {
                virtual_block_start = virtual_block_start.saturating_add(div_round_up(
                    u64::from(extent.length),
                    u64::from(self.logical_block_size),
                ));
                continue;
            }

            let block_count =
                div_round_up(u64::from(extent.length), u64::from(self.logical_block_size));
            out.push(FileExtent::new(
                virtual_block_start,
                extent.physical_lba,
                block_count,
            ));
            virtual_block_start = virtual_block_start.saturating_add(block_count);
        }

        Ok(out)
    }

    fn node_data_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        match node.flags & ICB_AD_MASK {
            ICB_AD_SHORT => self.short_extents(node),
            ICB_AD_LONG => self.long_extents(node),
            ICB_AD_IN_ICB => Ok(Vec::new()),
            ICB_AD_EXTENDED => Err(FsError::UnsupportedFs),
            _ => Err(FsError::UnsupportedFs),
        }
    }

    fn short_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        let descriptors =
            &node.entry[node.alloc_desc_offset..node.alloc_desc_offset + node.alloc_desc_len];
        let mut extents = Vec::new();
        for chunk in descriptors.chunks_exact(8) {
            let raw_length = read_u32(chunk, 0)?;
            let length = raw_length & EXTENT_LENGTH_MASK;
            if length == 0 {
                continue;
            }
            extents
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extents.push(NodeExtent {
                length,
                physical_lba: self.map_partition_block(node.part_ref, read_u32(chunk, 4)?)?,
                extent_type: raw_length & EXTENT_TYPE_MASK,
            });
        }
        Ok(extents)
    }

    fn long_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        let descriptors =
            &node.entry[node.alloc_desc_offset..node.alloc_desc_offset + node.alloc_desc_len];
        let mut extents = Vec::new();
        for chunk in descriptors.chunks_exact(16) {
            let raw_length = read_u32(chunk, 0)?;
            let length = raw_length & EXTENT_LENGTH_MASK;
            if length == 0 {
                continue;
            }
            let address = LogicalBlockAddress {
                block_num: read_u32(chunk, 4)?,
                part_ref: read_u16(chunk, 8)?,
            };
            extents
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extents.push(NodeExtent {
                length,
                physical_lba: self.map_logical_block(address)?,
                extent_type: raw_length & EXTENT_TYPE_MASK,
            });
        }
        Ok(extents)
    }

    fn node_start_lba(&self, node: &UdfNode) -> Option<u64> {
        self.node_extents(node)
            .ok()
            .and_then(|extents| extents.first().copied())
            .map(|extent| extent.physical_lba)
    }

    fn node_is_contiguous(&self, node: &UdfNode) -> bool {
        self.node_extents(node)
            .map(|extents| extents.len() <= 1)
            .unwrap_or(false)
    }

    fn map_logical_block(&self, address: LogicalBlockAddress) -> Result<u64, FsError> {
        self.map_partition_block(address.part_ref, address.block_num)
    }

    fn map_partition_block(&self, part_ref: u16, block_num: u32) -> Result<u64, FsError> {
        let map = self
            .partition_maps
            .get(part_ref as usize)
            .ok_or(FsError::Corrupted)?;
        let partition = self
            .partitions
            .get(map.partition_index)
            .ok_or(FsError::Corrupted)?;
        if block_num >= partition.length {
            return Err(FsError::ReadError);
        }
        Ok(u64::from(partition.start) + u64::from(block_num))
    }

    fn read_logical_block(&self, lba: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.logical_block_size as usize)?;
        self.read_full_logical_block(lba, &mut block)?;
        Ok(block)
    }

    fn read_full_logical_block(&self, lba: u64, block: &mut [u8]) -> Result<(), FsError> {
        if block.len() != self.logical_block_size as usize {
            return Err(FsError::InvalidArgument);
        }
        if self.logical_block_size != self.block_size {
            return Err(FsError::BlockSizeMismatch);
        }
        self.block_io.read_blocks(lba, block)
    }

    fn partition_descriptor_patch(
        &self,
        partition: Partition,
        replacement_end_lba: u64,
    ) -> Result<Option<UdfPartitionDescriptorPatch>, FsError> {
        let partition_end = u64::from(partition.start)
            .checked_add(u64::from(partition.length))
            .ok_or(FsError::Corrupted)?;
        if replacement_end_lba <= partition_end {
            return Ok(None);
        }

        let new_length = replacement_end_lba
            .checked_sub(u64::from(partition.start))
            .ok_or(FsError::Corrupted)?;
        let new_length = u32::try_from(new_length).map_err(|_| FsError::FileTooLarge)?;
        let mut descriptor = self.read_logical_block(partition.descriptor_lba)?;
        write_u32(&mut descriptor, PD_PARTITION_LENGTH_OFFSET, new_length)?;
        refresh_descriptor_tag(&mut descriptor)?;

        Ok(Some(UdfPartitionDescriptorPatch {
            descriptor_offset: partition
                .descriptor_lba
                .checked_mul(u64::from(self.logical_block_size))
                .ok_or(FsError::Corrupted)?,
            descriptor_data: descriptor,
        }))
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
