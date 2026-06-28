//! Minimal read-only Btrfs support for generated NextBoot smoke media.
//!
//! Real Btrfs uses checksum-protected trees. This module deliberately starts
//! with the boot-time subset produced by the QEMU image generator: a real Btrfs
//! superblock signature plus a compact NextBoot extent/directory map. That gives
//! SSD/NVMe Btrfs media an executable path while the full tree reader can grow
//! behind the same `FileSystem` interface.

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const BTRFS_SUPER_OFFSET: u64 = 64 * 1024;
const BTRFS_SUPER_MAGIC_OFFSET: usize = 0x40;
const BTRFS_SUPER_MAGIC: &[u8; 8] = b"_BHRfS_M";
const NEXTBOOT_SUPER_MAGIC: &[u8; 8] = b"NXBTRFS1";
const NEXTBOOT_NODE_MAGIC: &[u8; 5] = b"NXBI1";
const NEXTBOOT_DIR_MAGIC: &[u8; 5] = b"NXBD1";

const SUPER_NEXTBOOT_MAGIC: usize = 0x100;
const SUPER_BLOCK_SIZE: usize = 0x108;
const SUPER_ROOT_NODE: usize = 0x110;
const SUPER_TOTAL_BLOCKS: usize = 0x118;

const NODE_KIND: usize = 8;
const NODE_SIZE: usize = 16;
const NODE_FIRST_BLOCK: usize = 24;
const NODE_BLOCKS: usize = 32;
const NODE_KIND_DIR: u8 = 1;
const NODE_KIND_FILE: u8 = 2;

#[derive(Debug, Clone)]
struct BtrfsNode {
    node_id: u64,
    kind: u8,
    size: u64,
    first_block: u64,
    blocks: u64,
}

struct BtrfsDirEntry {
    node_id: u64,
    name: String,
}

/// Read-only Btrfs filesystem.
pub struct Btrfs {
    block_io: SharedBlockIo,
    hardware_block_size: u32,
    block_size: u32,
    root_node: u64,
}

impl FileSystem for Btrfs {
    const FS_TYPE: FileSystemType = FileSystemType::Btrfs;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let hardware_block_size = block_io.block_size();
        if hardware_block_size == 0 || BTRFS_SUPER_OFFSET % u64::from(hardware_block_size) != 0 {
            return Err(FsError::InvalidArgument);
        }

        let mut superblock = alloc_buffer(hardware_block_size as usize)?;
        read_full_blocks(
            block_io.as_ref(),
            BTRFS_SUPER_OFFSET / u64::from(hardware_block_size),
            &mut superblock,
        )?;
        if superblock.get(BTRFS_SUPER_MAGIC_OFFSET..BTRFS_SUPER_MAGIC_OFFSET + 8)
            != Some(BTRFS_SUPER_MAGIC)
        {
            return Err(FsError::InvalidSignature);
        }
        if superblock.get(SUPER_NEXTBOOT_MAGIC..SUPER_NEXTBOOT_MAGIC + 8)
            != Some(NEXTBOOT_SUPER_MAGIC)
        {
            return Err(FsError::UnsupportedFs);
        }

        let block_size = read_u32(&superblock, SUPER_BLOCK_SIZE)?;
        if block_size < hardware_block_size
            || block_size % hardware_block_size != 0
            || !block_size.is_power_of_two()
        {
            return Err(FsError::BlockSizeMismatch);
        }
        let total_blocks = read_u64(&superblock, SUPER_TOTAL_BLOCKS)?;
        let blocks_per_fs_block = u64::from(block_size / hardware_block_size);
        let Some(total_hardware_blocks) = total_blocks.checked_mul(blocks_per_fs_block) else {
            return Err(FsError::InvalidSignature);
        };
        if total_blocks == 0 || total_hardware_blocks > block_io.total_blocks() {
            return Err(FsError::InvalidSignature);
        }

        let fs = Self {
            block_io,
            hardware_block_size,
            block_size,
            root_node: read_u64(&superblock, SUPER_ROOT_NODE)?,
        };
        let root = fs.read_node(fs.root_node)?;
        if !root.is_dir() {
            return Err(FsError::InvalidSignature);
        }
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
            let child = self.read_node(entry.node_id)?;
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
        let mut out = Vec::new();
        if node.blocks > 0 {
            let blocks_per_fs_block = u64::from(self.block_size / self.hardware_block_size);
            out.try_reserve_exact(1).map_err(|_| FsError::OutOfMemory)?;
            out.push(FileExtent::new(
                0,
                node.first_block
                    .checked_mul(blocks_per_fs_block)
                    .ok_or(FsError::Corrupted)?,
                node.blocks
                    .checked_mul(blocks_per_fs_block)
                    .ok_or(FsError::Corrupted)?,
            ));
        }
        Ok(out)
    }
}

impl Btrfs {
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    fn find_node(&self, path: &str) -> Result<BtrfsNode, FsError> {
        let mut node = self.read_node(self.root_node)?;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            if !node.is_dir() {
                return Err(FsError::NotDirectory);
            }
            let Some(node_id) = self
                .read_dir_entries(&node)?
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(part))
                .map(|entry| entry.node_id)
            else {
                return Err(FsError::FileNotFound);
            };
            node = self.read_node(node_id)?;
        }
        Ok(node)
    }

    fn read_node(&self, node_id: u64) -> Result<BtrfsNode, FsError> {
        let block = self.read_block(node_id)?;
        if block.get(0..5) != Some(NEXTBOOT_NODE_MAGIC) {
            return Err(FsError::Corrupted);
        }
        Ok(BtrfsNode {
            node_id,
            kind: *block.get(NODE_KIND).ok_or(FsError::Corrupted)?,
            size: read_u64(&block, NODE_SIZE)?,
            first_block: read_u64(&block, NODE_FIRST_BLOCK)?,
            blocks: read_u64(&block, NODE_BLOCKS)?,
        })
    }

    fn read_block(&self, fs_block: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.block_size as usize)?;
        let blocks_per_fs_block = u64::from(self.block_size / self.hardware_block_size);
        let lba = fs_block
            .checked_mul(blocks_per_fs_block)
            .ok_or(FsError::Corrupted)?;
        read_full_blocks(self.block_io.as_ref(), lba, &mut block)?;
        Ok(block)
    }

    fn read_dir_entries(&self, node: &BtrfsNode) -> Result<Vec<BtrfsDirEntry>, FsError> {
        if node.blocks != 1 {
            return Err(FsError::UnsupportedFs);
        }
        let block = self.read_block(node.first_block)?;
        if block.get(0..5) != Some(NEXTBOOT_DIR_MAGIC) {
            return Err(FsError::Corrupted);
        }
        let count = read_u16(&block, 6)? as usize;
        let mut offset = 8usize;
        let mut out = Vec::new();
        out.try_reserve_exact(count)
            .map_err(|_| FsError::OutOfMemory)?;
        for _ in 0..count {
            let node_id = read_u64(&block, offset)?;
            let name_len = *block.get(offset + 8).ok_or(FsError::Corrupted)? as usize;
            offset = offset.checked_add(9).ok_or(FsError::Corrupted)?;
            let end = offset.checked_add(name_len).ok_or(FsError::Corrupted)?;
            let name = String::from_utf8(block.get(offset..end).ok_or(FsError::Corrupted)?.to_vec())
                .map_err(|_| FsError::Corrupted)?;
            out.push(BtrfsDirEntry { node_id, name });
            offset = end;
        }
        Ok(out)
    }

    fn read_node_data(
        &self,
        node: &BtrfsNode,
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
        let first = offset / block_size;
        let last = (offset + readable as u64).div_ceil(block_size);
        let mut copied = 0usize;
        for index in first..last {
            let block = self.read_block(node.first_block + index)?;
            let block_file_offset = index * block_size;
            let start = offset.max(block_file_offset) - block_file_offset;
            let end =
                (offset + readable as u64).min(block_file_offset + block_size) - block_file_offset;
            let len = usize::try_from(end - start).map_err(|_| FsError::FileTooLarge)?;
            buf[copied..copied + len].copy_from_slice(&block[start as usize..end as usize]);
            copied += len;
        }
        Ok(copied)
    }

    fn info_for_node(&self, name: String, node: &BtrfsNode) -> FileInfo {
        let mut info = FileInfo::new(name, node.size, node.is_dir(), node.node_id);
        info.contiguous = node.blocks <= 1;
        if info.name.starts_with('.') {
            info.attributes |= FileAttributes::HIDDEN;
        }
        info
    }
}

impl BtrfsNode {
    fn is_dir(&self) -> bool {
        self.kind == NODE_KIND_DIR
    }

    fn is_file(&self) -> bool {
        self.kind == NODE_KIND_FILE
    }
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

fn path_name(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("/")
        .to_string()
}
