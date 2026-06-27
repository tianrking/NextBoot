//! Minimal read-only XFS support for NextBoot QEMU data partitions.
//!
//! This is an intentionally small subset: XFS superblock, extent-format
//! dinodes, and a compact directory block used by the QEMU image generator.

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const XFS_SUPER_MAGIC: &[u8; 4] = b"XFSB";
const XFS_DINODE_MAGIC: u16 = 0x494E;
const XFS_DINODE_FMT_EXTENTS: u8 = 2;
const NEXTBOOT_XFS_DIR_MAGIC: &[u8; 4] = b"NXD1";

const SB_BLOCK_SIZE: usize = 4;
const SB_DBLOCKS: usize = 8;
const SB_ROOTINO: usize = 56;
const SB_INODE_SIZE: usize = 104;

const INODE_MAGIC: usize = 0;
const INODE_MODE: usize = 2;
const INODE_FORMAT: usize = 5;
const INODE_SIZE: usize = 56;
const INODE_NEXTENTS: usize = 76;
const INODE_DATA_FORK: usize = 100;

const XFS_S_IFMT: u16 = 0xF000;
const XFS_S_IFREG: u16 = 0x8000;
const XFS_S_IFDIR: u16 = 0x4000;

#[derive(Debug, Clone)]
struct XfsNode {
    inode_number: u64,
    mode: u16,
    size: u64,
    extents: Vec<XfsExtent>,
}

#[derive(Debug, Clone, Copy)]
struct XfsExtent {
    file_block: u64,
    physical_block: u64,
    block_count: u32,
}

pub struct Xfs {
    block_io: SharedBlockIo,
    block_size: u32,
    inode_size: u16,
    root_inode: u64,
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
        if block_size != hardware_block_size {
            return Err(FsError::BlockSizeMismatch);
        }
        let total_blocks = read_be_u64(&block, SB_DBLOCKS)?;
        if total_blocks == 0 || total_blocks > block_io.total_blocks() {
            return Err(FsError::InvalidSignature);
        }
        let inode_size = read_be_u16(&block, SB_INODE_SIZE)?;
        if inode_size < 128 || inode_size as u32 > block_size {
            return Err(FsError::UnsupportedFs);
        }
        let fs = Self {
            block_io,
            block_size,
            inode_size,
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
        self.block_size
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
        let block = self.read_block(inode_number)?;
        if read_be_u16(&block, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        if block[INODE_FORMAT] != XFS_DINODE_FMT_EXTENTS {
            return Err(FsError::UnsupportedFs);
        }
        let nextents = read_be_u32(&block, INODE_NEXTENTS)? as usize;
        let extent_end = INODE_DATA_FORK
            .checked_add(nextents.checked_mul(16).ok_or(FsError::Corrupted)?)
            .ok_or(FsError::Corrupted)?;
        if extent_end > self.inode_size as usize || extent_end > block.len() {
            return Err(FsError::Corrupted);
        }

        let mut extents = Vec::new();
        extents
            .try_reserve_exact(nextents)
            .map_err(|_| FsError::OutOfMemory)?;
        for index in 0..nextents {
            let offset = INODE_DATA_FORK + index * 16;
            extents.push(read_bmbt_record(&block[offset..offset + 16])?);
        }
        Ok(XfsNode {
            inode_number,
            mode: read_be_u16(&block, INODE_MODE)?,
            size: read_be_u64(&block, INODE_SIZE)?,
            extents,
        })
    }

    fn read_block(&self, fs_block: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.block_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), fs_block, &mut block)?;
        Ok(block)
    }

    fn read_dir_entries(&self, node: &XfsNode) -> Result<Vec<XfsDirEntry>, FsError> {
        let mut data =
            alloc_buffer(usize::try_from(node.size).map_err(|_| FsError::FileTooLarge)?)?;
        if !data.is_empty() {
            self.read_node_data(node, 0, &mut data)?;
        }
        if data.get(0..4) != Some(NEXTBOOT_XFS_DIR_MAGIC) {
            return Err(FsError::UnsupportedFs);
        }
        let count = read_be_u16(&data, 4)? as usize;
        let mut offset = 6usize;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| FsError::OutOfMemory)?;
        for _ in 0..count {
            let inode_number = read_be_u64(&data, offset)?;
            let name_len = *data.get(offset + 8).ok_or(FsError::Corrupted)? as usize;
            offset = offset.checked_add(9).ok_or(FsError::Corrupted)?;
            let name_end = offset.checked_add(name_len).ok_or(FsError::Corrupted)?;
            let name = String::from_utf8(
                data.get(offset..name_end)
                    .ok_or(FsError::Corrupted)?
                    .to_vec(),
            )
            .map_err(|_| FsError::Corrupted)?;
            entries.push(XfsDirEntry { inode_number, name });
            offset = name_end;
        }
        Ok(entries)
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
        for extent in &node.extents {
            out.push(FileExtent::new(
                extent.file_block,
                extent.physical_block,
                u64::from(extent.block_count),
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

fn read_be_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::Corrupted)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_be_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::Corrupted)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_be_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 8).ok_or(FsError::Corrupted)?;
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
