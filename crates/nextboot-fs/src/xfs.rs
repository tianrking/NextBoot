//! Minimal read-only XFS support for NextBoot data partitions.
//!
//! This covers the small boot-time subset NextBoot needs: superblock parsing,
//! extent-format regular files, local and dir2 block/data directories, plus
//! the compact directory block used by the QEMU image generator.

mod directories;
mod parse;

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use parse::{nonzero_log2, read_be_u16, read_be_u32, read_be_u64, read_log_or_ceil};

const XFS_SUPER_MAGIC: &[u8; 4] = b"XFSB";
const XFS_DINODE_MAGIC: u16 = 0x494E;
const XFS_DINODE_FMT_LOCAL: u8 = 1;
const XFS_DINODE_FMT_EXTENTS: u8 = 2;
const XFS_SB_VERSION_5: u16 = 5;
const XFS_SB_VERSION_NUMBITS: u16 = 0x000f;
const XFS_SB_FEAT_INCOMPAT_FTYPE: u32 = 0x0000_0001;
const XFS_SB_VERSION2_FTYPE: u32 = 0x0000_0200;

const SB_BLOCK_SIZE: usize = 4;
const SB_DBLOCKS: usize = 8;
const SB_ROOTINO: usize = 56;
const SB_AGBLOCKS: usize = 84;
const SB_INODE_SIZE: usize = 104;
const SB_INOPBLOCK: usize = 106;
const SB_VERSION: usize = 100;
const SB_INOPBLOG: usize = 123;
const SB_AGBLKLOG: usize = 124;
const SB_DIRBLKLOG: usize = 192;
const SB_FEATURES2: usize = 200;
const SB_FEATURES_INCOMPAT: usize = 216;

const INODE_MAGIC: usize = 0;
const INODE_MODE: usize = 2;
const INODE_VERSION: usize = 4;
const INODE_FORMAT: usize = 5;
const INODE_SIZE: usize = 56;
const INODE_NEXTENTS: usize = 76;
const INODE_V2_DATA_FORK: usize = 100;
const INODE_V3_DATA_FORK: usize = 176;

const XFS_S_IFMT: u16 = 0xF000;
const XFS_S_IFREG: u16 = 0x8000;
const XFS_S_IFDIR: u16 = 0x4000;

#[derive(Debug, Clone)]
struct XfsNode {
    inode_number: u64,
    mode: u16,
    size: u64,
    format: u8,
    extents: Vec<XfsExtent>,
    local_data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct XfsExtent {
    file_block: u64,
    physical_block: u64,
    block_count: u32,
}

pub struct Xfs {
    block_io: SharedBlockIo,
    hardware_block_size: u32,
    block_size: u32,
    inode_size: u16,
    root_inode: u64,
    agblocks: u32,
    inopblog: u8,
    inode_agino_bits: u8,
    dir_block_size: u32,
    has_ftype: bool,
}

impl FileSystem for Xfs {
    const FS_TYPE: FileSystemType = FileSystemType::Xfs;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let hardware_block_size = block_io.block_size();
        if hardware_block_size == 0 {
            return Err(FsError::InvalidArgument);
        }
        let mut block = alloc_buffer(hardware_block_size as usize)?;
        read_full_blocks(block_io.as_ref(), 0, &mut block)?;
        if block.get(0..4) != Some(XFS_SUPER_MAGIC) {
            return Err(FsError::InvalidSignature);
        }

        let block_size = read_be_u32(&block, SB_BLOCK_SIZE)?;
        if block_size < hardware_block_size
            || block_size % hardware_block_size != 0
            || !block_size.is_power_of_two()
        {
            return Err(FsError::BlockSizeMismatch);
        }
        let fs_block_ratio = u64::from(block_size / hardware_block_size);
        let total_blocks = read_be_u64(&block, SB_DBLOCKS)?;
        let Some(total_hardware_blocks) = total_blocks.checked_mul(fs_block_ratio) else {
            return Err(FsError::InvalidSignature);
        };
        if total_blocks == 0 || total_hardware_blocks > block_io.total_blocks() {
            return Err(FsError::InvalidSignature);
        }
        let inode_size = read_be_u16(&block, SB_INODE_SIZE)?;
        if inode_size < 128 || inode_size as u32 > block_size {
            return Err(FsError::UnsupportedFs);
        }
        let agblocks = read_be_u32(&block, SB_AGBLOCKS)?;
        let inopblock = read_be_u16(&block, SB_INOPBLOCK)?;
        if agblocks == 0 || inopblock == 0 || !inopblock.is_power_of_two() {
            return Err(FsError::InvalidSignature);
        }
        let version = read_be_u16(&block, SB_VERSION)? & XFS_SB_VERSION_NUMBITS;
        let features2 = read_be_u32(&block, SB_FEATURES2).unwrap_or(0);
        let incompat = read_be_u32(&block, SB_FEATURES_INCOMPAT).unwrap_or(0);
        let has_ftype = (version >= XFS_SB_VERSION_5 && incompat & XFS_SB_FEAT_INCOMPAT_FTYPE != 0)
            || features2 & XFS_SB_VERSION2_FTYPE != 0;
        let inopblog = nonzero_log2(inopblock as u32).ok_or(FsError::InvalidSignature)?;
        let agblklog = read_log_or_ceil(&block, SB_AGBLKLOG, agblocks);
        let dirblklog = block.get(SB_DIRBLKLOG).copied().unwrap_or(0);
        let dir_block_size = block_size
            .checked_shl(u32::from(dirblklog))
            .ok_or(FsError::UnsupportedFs)?;
        let fs = Self {
            block_io,
            hardware_block_size,
            block_size,
            inode_size,
            agblocks,
            inopblog: block
                .get(SB_INOPBLOG)
                .copied()
                .unwrap_or(inopblog)
                .max(inopblog),
            inode_agino_bits: agblklog
                .checked_add(inopblog)
                .ok_or(FsError::InvalidSignature)?,
            dir_block_size,
            has_ftype,
            root_inode: read_be_u64(&block, SB_ROOTINO)?,
        };
        fs.read_inode(fs.root_inode)?;
        Ok(fs)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        let node = self.find_node(path)?;
        if !node.is_dir() {
            return Err(FsError::NotDirectory);
        }
        let entries = self.read_dir_entries(&node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(entries.len())
            .map_err(|_| FsError::OutOfMemory)?;
        for entry in entries {
            let child = self.read_inode(entry.inode_number)?;
            out.push(self.info_for_node(entry.name, &child));
        }
        Ok(out)
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }
        self.read_node_data(&node, offset, buf)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        let node = self.find_node(path)?;
        Ok(self.info_for_node(path_name(path), &node))
    }

    fn block_size(&self) -> u32 {
        self.hardware_block_size
    }

    fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }
        self.file_extents_for_node(&node)
    }
}

impl Xfs {
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    fn find_node(&self, path: &str) -> Result<XfsNode, FsError> {
        let mut node = self.read_inode(self.root_inode)?;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            if !node.is_dir() {
                return Err(FsError::NotDirectory);
            }
            let Some(inode_number) = self
                .read_dir_entries(&node)?
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(part))
                .map(|entry| entry.inode_number)
            else {
                return Err(FsError::FileNotFound);
            };
            node = self.read_inode(inode_number)?;
        }
        Ok(node)
    }

    fn read_inode(&self, inode_number: u64) -> Result<XfsNode, FsError> {
        if inode_number == 0 {
            return Err(FsError::Corrupted);
        }
        let block = self.read_inode_bytes(inode_number)?;
        if read_be_u16(&block, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        let format = block[INODE_FORMAT];
        if !matches!(format, XFS_DINODE_FMT_LOCAL | XFS_DINODE_FMT_EXTENTS) {
            return Err(FsError::UnsupportedFs);
        }
        let data_fork = if block[INODE_VERSION] >= 3 {
            INODE_V3_DATA_FORK
        } else {
            INODE_V2_DATA_FORK
        };
        if data_fork > self.inode_size as usize || data_fork > block.len() {
            return Err(FsError::Corrupted);
        }
        let size = read_be_u64(&block, INODE_SIZE)?;
        let mut local_data = Vec::new();
        if format == XFS_DINODE_FMT_LOCAL {
            let local_len = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
            let local_end = data_fork.checked_add(local_len).ok_or(FsError::Corrupted)?;
            if local_end > self.inode_size as usize || local_end > block.len() {
                return Err(FsError::Corrupted);
            }
            local_data
                .try_reserve_exact(local_len)
                .map_err(|_| FsError::OutOfMemory)?;
            local_data.extend_from_slice(&block[data_fork..local_end]);
        }
        let nextents = read_be_u32(&block, INODE_NEXTENTS)? as usize;
        if format == XFS_DINODE_FMT_LOCAL && nextents != 0 {
            return Err(FsError::Corrupted);
        }

        let mut extents = Vec::new();
        if format == XFS_DINODE_FMT_EXTENTS {
            let extent_end = data_fork
                .checked_add(nextents.checked_mul(16).ok_or(FsError::Corrupted)?)
                .ok_or(FsError::Corrupted)?;
            if extent_end > self.inode_size as usize || extent_end > block.len() {
                return Err(FsError::Corrupted);
            }
            extents
                .try_reserve_exact(nextents)
                .map_err(|_| FsError::OutOfMemory)?;
            for index in 0..nextents {
                let offset = data_fork + index * 16;
                extents.push(read_bmbt_record(&block[offset..offset + 16])?);
            }
        }
        Ok(XfsNode {
            inode_number,
            mode: read_be_u16(&block, INODE_MODE)?,
            size,
            format,
            extents,
            local_data,
        })
    }

    fn read_block(&self, fs_block: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.block_size as usize)?;
        let hardware_blocks = u64::from(self.block_size / self.hardware_block_size);
        let lba = fs_block
            .checked_mul(hardware_blocks)
            .ok_or(FsError::Corrupted)?;
        read_full_blocks(self.block_io.as_ref(), lba, &mut block)?;
        Ok(block)
    }

    fn read_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        if let Ok(inode) = self.read_mapped_inode_bytes(inode_number) {
            return Ok(inode);
        }
        self.read_legacy_inode_bytes(inode_number)
    }

    fn read_mapped_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        if self.inode_agino_bits >= 64 {
            return Err(FsError::Corrupted);
        }
        let agino_mask = (1u64 << self.inode_agino_bits) - 1;
        let agno = inode_number >> self.inode_agino_bits;
        let agino = inode_number & agino_mask;
        let agbno = agino >> self.inopblog;
        let inode_index = agino & ((1u64 << self.inopblog) - 1);
        let fs_block = agno
            .checked_mul(u64::from(self.agblocks))
            .and_then(|base| base.checked_add(agbno))
            .ok_or(FsError::Corrupted)?;
        let block = self.read_block(fs_block)?;
        let start = usize::try_from(inode_index)
            .ok()
            .and_then(|index| index.checked_mul(self.inode_size as usize))
            .ok_or(FsError::Corrupted)?;
        let end = start
            .checked_add(self.inode_size as usize)
            .ok_or(FsError::Corrupted)?;
        let inode = block.get(start..end).ok_or(FsError::Corrupted)?.to_vec();
        if read_be_u16(&inode, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        Ok(inode)
    }

    fn read_legacy_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        let block = self.read_block(inode_number)?;
        let inode = block
            .get(..self.inode_size as usize)
            .ok_or(FsError::Corrupted)?
            .to_vec();
        if read_be_u16(&inode, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        Ok(inode)
    }

    fn read_dir_entries(&self, node: &XfsNode) -> Result<Vec<XfsDirEntry>, FsError> {
        directories::read_dir_entries(self, node)
    }

    fn read_node_data(
        &self,
        node: &XfsNode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= node.size || buf.is_empty() {
            return Ok(0);
        }
        if node.format == XFS_DINODE_FMT_LOCAL {
            let start = usize::try_from(offset).map_err(|_| FsError::FileTooLarge)?;
            let readable = buf.len().min(node.local_data.len().saturating_sub(start));
            buf[..readable].copy_from_slice(&node.local_data[start..start + readable]);
            return Ok(readable);
        }
        let readable = buf
            .len()
            .min(usize::try_from(node.size - offset).map_err(|_| FsError::FileTooLarge)?);
        let block_size = u64::from(self.block_size);
        let mut copied = 0usize;
        for extent in &node.extents {
            let extent_start = extent.file_block * block_size;
            let extent_end = extent_start + u64::from(extent.block_count) * block_size;
            if offset >= extent_end || offset + readable as u64 <= extent_start {
                continue;
            }
            let read_start = offset.max(extent_start);
            let read_end = (offset + readable as u64).min(extent_end);
            let first = (read_start - extent_start) / block_size;
            let last = (read_end - extent_start).div_ceil(block_size);
            for index in first..last {
                let block = self.read_block(extent.physical_block + index)?;
                let block_file_offset = extent_start + index * block_size;
                let start = read_start.max(block_file_offset) - block_file_offset;
                let end = read_end.min(block_file_offset + block_size) - block_file_offset;
                let len = usize::try_from(end - start).map_err(|_| FsError::FileTooLarge)?;
                buf[copied..copied + len].copy_from_slice(&block[start as usize..end as usize]);
                copied += len;
            }
        }
        Ok(copied)
    }

    fn file_extents_for_node(&self, node: &XfsNode) -> Result<Vec<FileExtent>, FsError> {
        let mut out = Vec::new();
        out.try_reserve_exact(node.extents.len())
            .map_err(|_| FsError::OutOfMemory)?;
        let blocks_per_fs_block = u64::from(self.block_size / self.hardware_block_size);
        for extent in &node.extents {
            out.push(FileExtent::new(
                extent
                    .file_block
                    .checked_mul(blocks_per_fs_block)
                    .ok_or(FsError::Corrupted)?,
                extent
                    .physical_block
                    .checked_mul(blocks_per_fs_block)
                    .ok_or(FsError::Corrupted)?,
                u64::from(extent.block_count)
                    .checked_mul(blocks_per_fs_block)
                    .ok_or(FsError::Corrupted)?,
            ));
        }
        Ok(out)
    }

    fn info_for_node(&self, name: String, node: &XfsNode) -> FileInfo {
        let mut info = FileInfo::new(name, node.size, node.is_dir(), node.inode_number);
        info.contiguous = node.extents.len() <= 1;
        if info.name.starts_with('.') {
            info.attributes |= FileAttributes::HIDDEN;
        }
        info
    }
}

impl XfsNode {
    fn is_dir(&self) -> bool {
        self.mode & XFS_S_IFMT == XFS_S_IFDIR
    }

    fn is_file(&self) -> bool {
        self.mode & XFS_S_IFMT == XFS_S_IFREG
    }
}

struct XfsDirEntry {
    inode_number: u64,
    name: String,
}

fn read_bmbt_record(data: &[u8]) -> Result<XfsExtent, FsError> {
    let l0 = read_be_u64(data, 0)?;
    let l1 = read_be_u64(data, 8)?;
    if l0 >> 63 != 0 {
        return Err(FsError::UnsupportedFs);
    }
    let file_block = (l0 >> 9) & ((1u64 << 54) - 1);
    let physical_block = ((l0 & 0x1FF) << 43) | (l1 >> 21);
    let block_count = (l1 & ((1u64 << 21) - 1)) as u32;
    if block_count == 0 {
        return Err(FsError::Corrupted);
    }
    Ok(XfsExtent {
        file_block,
        physical_block,
        block_count,
    })
}

fn path_name(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("/")
        .to_string()
}
