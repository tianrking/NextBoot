//! exFAT 文件系统实现
//!
//! 用于 Data 分区，支持 >4GB 文件

use crate::{FileSystem, FileSystemType, FileInfo, FsError, BlockIoOps};
use alloc::vec::Vec;

/// exFAT 文件系统
pub struct ExFat {
    block_size: u32,
    cluster_size: u32,
    total_clusters: u32,
    root_cluster: u32,
}

/// exFAT 引导扇区
#[repr(C, packed)]
struct ExFatBootSector {
    jump: [u8; 3],
    fs_name: [u8; 8],
    reserved1: [u8; 53],
    partition_offset: u64,
    volume_length: u64,
    fat_offset: u32,
    fat_length: u32,
    cluster_heap_offset: u32,
    cluster_count: u32,
    root_cluster: u32,
    volume_serial: u32,
    fs_revision: u16,
    volume_flags: u16,
    bytes_per_sector: u8,
    sectors_per_cluster: u8,
    num_fats: u8,
    drive_select: u8,
    percent_in_use: u8,
    reserved2: [u8; 7],
}

impl FileSystem for ExFat {
    const FS_TYPE: FileSystemType = FileSystemType::ExFat;

    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError> {
        let mut boot_buf = [0u8; 512];
        block_io.read_blocks(0, &mut boot_buf)?;

        let boot: ExFatBootSector = unsafe {
            core::mem::transmute_copy(&boot_buf)
        };

        // 验证 exFAT 签名
        if &boot.fs_name != b"EXFAT   " {
            return Err(FsError::InvalidSignature);
        }

        let sector_size = 1u32 << boot.bytes_per_sector;
        let cluster_size = sector_size << boot.sectors_per_cluster;

        Ok(Self {
            block_size: sector_size,
            cluster_size,
            total_clusters: boot.cluster_count,
            root_cluster: boot.root_cluster,
        })
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        // TODO: 实现 exFAT 目录读取
        // exFAT 使用 UTF-16LE 文件名
        Ok(Vec::new())
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        // TODO: 实现文件读取
        Ok(0)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        Err(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }
}

/// exFAT 目录条目
#[derive(Debug, Clone)]
#[repr(u8)]
pub enum ExFatEntry {
    /// 主条目 (0x85)
    File = 0x85,
    /// 流扩展条目 (0xC0)
    Stream = 0xC0,
    /// 文件名条目 (0xC1)
    Name = 0xC1,
}
