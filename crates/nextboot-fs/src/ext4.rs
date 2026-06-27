//! Minimal read-only ext4 support.
//!
//! This implements the subset needed for NextBoot data partitions: 4K ext4
//! block size, extent-backed regular files, and linear directory entries.

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const EXT4_SUPER_OFFSET: usize = 1024;
const EXT4_SUPER_MAGIC: u16 = 0xEF53;
const EXT4_ROOT_INO: u32 = 2;
const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
const EXT4_EXTENT_MAGIC: u16 = 0xF30A;

const S_LOG_BLOCK_SIZE: usize = 24;
const S_BLOCKS_PER_GROUP: usize = 32;
const S_INODES_PER_GROUP: usize = 40;
const S_MAGIC: usize = 56;
const S_INODE_SIZE: usize = 88;
const S_DESC_SIZE: usize = 254;

const GD_INODE_TABLE_LO: usize = 8;
const INODE_MODE: usize = 0;
const INODE_SIZE_LO: usize = 4;
const INODE_FLAGS: usize = 32;
const INODE_BLOCKS: usize = 40;
const INODE_SIZE_HIGH: usize = 108;

const EXTENT_HEADER_ENTRIES: usize = 2;
const EXTENT_HEADER_DEPTH: usize = 6;
const EXTENT_ENTRY_SIZE: usize = 12;
const EXTENT_ENTRY_OFFSET: usize = 12;

const EXT4_S_IFMT: u16 = 0xF000;
const EXT4_S_IFREG: u16 = 0x8000;
const EXT4_S_IFDIR: u16 = 0x4000;

#[derive(Debug, Clone)]
struct Ext4Node {
    inode_number: u32,
    mode: u16,
    flags: u32,
    size: u64,
    inode: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Ext4Extent {
    file_block: u32,
    block_count: u16,
    physical_block: u64,
}

/// Read-only ext4 filesystem.
pub struct Ext4 {
    block_io: SharedBlockIo,
    block_size: u32,
    inode_size: u16,
    inodes_per_group: u32,
    group_desc_size: u16,
    group_desc_lba: u64,
}

impl FileSystem for Ext4 {
    const FS_TYPE: FileSystemType = FileSystemType::Ext4;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let hardware_block_size = block_io.block_size();
        if hardware_block_size == 0 {
            return Err(FsError::InvalidArgument);
        }

        let mut first_block = alloc_buffer(hardware_block_size as usize)?;
        read_full_blocks(block_io.as_ref(), 0, &mut first_block)?;
        if first_block.len() < EXT4_SUPER_OFFSET + 1024 {
            return Err(FsError::BlockSizeMismatch);
        }

        let superblock = &first_block[EXT4_SUPER_OFFSET..EXT4_SUPER_OFFSET + 1024];
        if read_u16(superblock, S_MAGIC)? != EXT4_SUPER_MAGIC {
            return Err(FsError::InvalidSignature);
        }

        let block_size = 1024u32
            .checked_shl(read_u32(superblock, S_LOG_BLOCK_SIZE)?)
            .ok_or(FsError::UnsupportedFs)?;
        if block_size != hardware_block_size {
            return Err(FsError::BlockSizeMismatch);
        }

        let inode_size = read_u16(superblock, S_INODE_SIZE)?;
        if inode_size < 128 || inode_size as u32 > block_size {
            return Err(FsError::UnsupportedFs);
        }
        let inodes_per_group = read_u32(superblock, S_INODES_PER_GROUP)?;
        if inodes_per_group == 0 || read_u32(superblock, S_BLOCKS_PER_GROUP)? == 0 {
            return Err(FsError::InvalidSignature);
        }

        let raw_desc_size = read_u16(superblock, S_DESC_SIZE)?;
        let group_desc_size = raw_desc_size.max(32);
        let fs = Self {
            block_io,
            block_size,
            inode_size,
            inodes_per_group,
            group_desc_size,
            group_desc_lba: 1,
        };
        fs.read_inode(EXT4_ROOT_INO)?;
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

impl Ext4 {
    /// Open an ext4 filesystem from a shared block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    fn find_node(&self, path: &str) -> Result<Ext4Node, FsError> {
        let mut node = self.read_inode(EXT4_ROOT_INO)?;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            if !node.is_dir() {
                return Err(FsError::NotDirectory);
            }
            let entries = self.read_dir_entries(&node)?;
            let Some(inode_number) = entries
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

    fn read_inode(&self, inode_number: u32) -> Result<Ext4Node, FsError> {
        if inode_number == 0 {
            return Err(FsError::Corrupted);
        }

        let group = (inode_number - 1) / self.inodes_per_group;
        let index = (inode_number - 1) % self.inodes_per_group;
        let inode_table = self.read_inode_table_block(group)?;
        let inode_offset = u64::from(index)
            .checked_mul(u64::from(self.inode_size))
            .ok_or(FsError::Corrupted)?;
        let inode_lba = inode_table
            .checked_add(inode_offset / u64::from(self.block_size))
            .ok_or(FsError::Corrupted)?;
        let inode_in_block = (inode_offset % u64::from(self.block_size)) as usize;

        let block = self.read_block(inode_lba)?;
        let end = inode_in_block
            .checked_add(self.inode_size as usize)
            .ok_or(FsError::Corrupted)?;
        if end > block.len() {
            return Err(FsError::Corrupted);
        }
        let inode = block[inode_in_block..end].to_vec();
        let size = u64::from(read_u32(&inode, INODE_SIZE_LO)?)
            | (u64::from(read_u32(&inode, INODE_SIZE_HIGH)?) << 32);

        Ok(Ext4Node {
            inode_number,
            mode: read_u16(&inode, INODE_MODE)?,
            flags: read_u32(&inode, INODE_FLAGS)?,
            size,
            inode,
        })
    }

    fn read_inode_table_block(&self, group: u32) -> Result<u64, FsError> {
        let desc_offset = u64::from(group)
            .checked_mul(u64::from(self.group_desc_size))
            .ok_or(FsError::Corrupted)?;
        let desc_lba = self
            .group_desc_lba
            .checked_add(desc_offset / u64::from(self.block_size))
            .ok_or(FsError::Corrupted)?;
        let desc_in_block = (desc_offset % u64::from(self.block_size)) as usize;
        let block = self.read_block(desc_lba)?;
        let desc_end = desc_in_block
            .checked_add(self.group_desc_size as usize)
            .ok_or(FsError::Corrupted)?;
        if desc_end > block.len() {
            return Err(FsError::Corrupted);
        }
        Ok(u64::from(read_u32(
            &block,
            desc_in_block + GD_INODE_TABLE_LO,
        )?))
    }

    fn read_block(&self, fs_block: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.block_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), fs_block, &mut block)?;
        Ok(block)
    }

    fn read_dir_node(&self, node: &Ext4Node) -> Result<Vec<FileInfo>, FsError> {
        let entries = self.read_dir_entries(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(entries.len())
            .map_err(|_| FsError::OutOfMemory)?;
        for entry in entries {
            let child = self.read_inode(entry.inode_number)?;
            out.push(self.info_for_node(entry.name, &child));
        }
        Ok(out)
    }

    fn read_dir_entries(&self, node: &Ext4Node) -> Result<Vec<Ext4DirEntry>, FsError> {
        let mut data =
            alloc_buffer(usize::try_from(node.size).map_err(|_| FsError::FileTooLarge)?)?;
        if !data.is_empty() {
            self.read_node_data(node, 0, &mut data)?;
        }

        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset + 8 <= data.len() {
            let inode_number = read_u32(&data, offset)?;
            let rec_len = read_u16(&data, offset + 4)? as usize;
            let name_len = data[offset + 6] as usize;
            if rec_len < 8 || offset + rec_len > data.len() || name_len > rec_len - 8 {
                return Err(FsError::Corrupted);
            }
            if inode_number != 0 {
                let name = String::from_utf8(data[offset + 8..offset + 8 + name_len].to_vec())
                    .map_err(|_| FsError::Corrupted)?;
                if name != "." && name != ".." {
                    entries.push(Ext4DirEntry { inode_number, name });
                }
            }
            offset += rec_len;
        }
        Ok(entries)
    }

    fn read_node_data(
        &self,
        node: &Ext4Node,
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
        for extent in self.extents(node)? {
            let extent_start = u64::from(extent.file_block) * block_size;
            let extent_len = u64::from(extent.block_count) * block_size;
            let extent_end = extent_start.saturating_add(extent_len);
            if offset >= extent_end || offset + readable as u64 <= extent_start {
                continue;
            }
            let read_start = offset.max(extent_start);
            let read_end = (offset + readable as u64).min(extent_end);
            let first_block = (read_start - extent_start) / block_size;
            let last_block = (read_end - extent_start + block_size - 1) / block_size;
            let mut block_index = first_block;
            while block_index < last_block && copied < readable {
                let block = self.read_block(extent.physical_block + block_index)?;
                let block_file_offset = extent_start + block_index * block_size;
                let start = read_start.max(block_file_offset) - block_file_offset;
                let end = read_end.min(block_file_offset + block_size) - block_file_offset;
                let len = usize::try_from(end - start).map_err(|_| FsError::FileTooLarge)?;
                buf[copied..copied + len].copy_from_slice(&block[start as usize..end as usize]);
                copied += len;
                block_index += 1;
            }
        }
        Ok(copied)
    }

    fn file_extents_for_node(&self, node: &Ext4Node) -> Result<Vec<FileExtent>, FsError> {
        let extents = self.extents(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(extents.len())
            .map_err(|_| FsError::OutOfMemory)?;
        for extent in extents {
            out.push(FileExtent::new(
                u64::from(extent.file_block),
                extent.physical_block,
                u64::from(extent.block_count),
            ));
        }
        Ok(out)
    }

    fn extents(&self, node: &Ext4Node) -> Result<Vec<Ext4Extent>, FsError> {
        if node.flags & EXT4_EXTENTS_FL == 0 {
            return Err(FsError::UnsupportedFs);
        }
        let root = &node.inode[INODE_BLOCKS..INODE_BLOCKS + 60];
        if read_u16(root, 0)? != EXT4_EXTENT_MAGIC || read_u16(root, EXTENT_HEADER_DEPTH)? != 0 {
            return Err(FsError::UnsupportedFs);
        }
        let entries = read_u16(root, EXTENT_HEADER_ENTRIES)? as usize;
        if EXTENT_ENTRY_OFFSET + entries * EXTENT_ENTRY_SIZE > root.len() {
            return Err(FsError::Corrupted);
        }
        let mut out = Vec::new();
        for index in 0..entries {
            let offset = EXTENT_ENTRY_OFFSET + index * EXTENT_ENTRY_SIZE;
            let block_count = read_u16(root, offset + 4)? & 0x7FFF;
            if block_count == 0 {
                continue;
            }
            out.push(Ext4Extent {
                file_block: read_u32(root, offset)?,
                block_count,
                physical_block: (u64::from(read_u16(root, offset + 6)?) << 32)
                    | u64::from(read_u32(root, offset + 8)?),
            });
        }
        Ok(out)
    }

    fn info_for_node(&self, name: String, node: &Ext4Node) -> FileInfo {
        let mut info = FileInfo::new(name, node.size, node.is_dir(), u64::from(node.inode_number));
        info.contiguous = self
            .extents(node)
            .map(|extents| extents.len() <= 1)
            .unwrap_or(false);
        if info.name.starts_with('.') {
            info.attributes |= FileAttributes::HIDDEN;
        }
        info
    }
}

impl Ext4Node {
    fn is_dir(&self) -> bool {
        self.mode & EXT4_S_IFMT == EXT4_S_IFDIR
    }

    fn is_file(&self) -> bool {
        self.mode & EXT4_S_IFMT == EXT4_S_IFREG
    }
}

struct Ext4DirEntry {
    inode_number: u32,
    name: String,
}

fn path_name(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("/")
        .to_string()
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::Corrupted)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::Corrupted)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
