//! FAT32 文件系统实现
//!
//! 仅支持读取，用于 ESP 分区

use crate::{FileSystem, FileSystemType, FileInfo, FsError, BlockIoOps};
use alloc::vec::Vec;
use alloc::string::String;
use byteorder::{LittleEndian, ByteOrder};

/// FAT32 文件系统
pub struct Fat32 {
    block_io: alloc::boxed::Box<dyn BlockIoOps>,
    boot_sector: Fat32BootSector,
    fat: Vec<u32>,
    cluster_size: u32,
    root_cluster: u32,
}

/// FAT32 引导扇区
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Fat32BootSector {
    jump: [u8; 3],
    oem: [u8; 8],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    root_entries: u16,
    total_sectors_16: u16,
    media_type: u8,
    sectors_per_fat_16: u16,
    sectors_per_track: u16,
    num_heads: u16,
    hidden_sectors: u32,
    total_sectors_32: u32,
    // FAT32 扩展
    sectors_per_fat_32: u32,
    ext_flags: u16,
    fs_version: u16,
    root_cluster: u32,
    fs_info_sector: u16,
    backup_boot_sector: u16,
    reserved: [u8; 12],
    drive_num: u8,
    reserved1: u8,
    boot_signature: u8,
    volume_id: u32,
    volume_label: [u8; 11],
    fs_type: [u8; 8],
}

impl FileSystem for Fat32 {
    const FS_TYPE: FileSystemType = FileSystemType::Fat32;

    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError> {
        let mut boot_buf = [0u8; 512];
        block_io.read_blocks(0, &mut boot_buf)?;

        // 安全转换
        let boot_sector: Fat32BootSector = unsafe {
            core::mem::transmute_copy(&boot_buf)
        };

        // 验证 FAT32 签名
        if &boot_sector.fs_type != b"FAT32   " {
            return Err(FsError::InvalidSignature);
        }

        let cluster_size = boot_sector.bytes_per_sector as u32
            * boot_sector.sectors_per_cluster as u32;

        // 读取 FAT 表
        let fat_start = boot_sector.reserved_sectors as u64;
        let fat_size = boot_sector.sectors_per_fat_32 as u64;
        let bytes_per_sector = boot_sector.bytes_per_sector as u64;

        let mut fat = Vec::new();
        // 简化: 只读取 FAT 的一部分 (实际需要完整读取)
        let fat_entries = (fat_size * bytes_per_sector / 4) as usize;
        fat.reserve(fat_entries.min(65536)); // 限制内存使用

        Ok(Self {
            // 注意: 这里需要某种方式持有 block_io 引用
            // 实际实现可能需要使用引用计数或其他方案
            block_io: alloc::boxed::Box::new(PlaceholderBlockIo),
            boot_sector,
            fat,
            cluster_size,
            root_cluster: boot_sector.root_cluster,
        })
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        // TODO: 实现目录读取
        Ok(Vec::new())
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        // TODO: 实现文件读取
        Ok(0)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        // TODO: 实现文件信息获取
        Err(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.boot_sector.bytes_per_sector as u32
    }
}

// 占位符 - 实际实现需要解决 BlockIO 所有权问题
struct PlaceholderBlockIo;

impl BlockIoOps for PlaceholderBlockIo {
    fn block_size(&self) -> u32 { 512 }
    fn total_blocks(&self) -> u64 { 0 }
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        Err(FsError::ReadError)
    }
}
