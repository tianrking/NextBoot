//! NextBoot 文件系统模块
//!
//! 提供 FAT32、exFAT 和 ISO9660 文件系统的只读支持。
//!
//! # 设计原则
//! - 所有操作都是只读的 (符合 PRD 要求)
//! - 支持动态块大小检测 (4K/512B)
//! - 零拷贝设计，最小化内存使用

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod fat32;
pub mod exfat;
pub mod iso9660;
pub mod gpt;

/// 文件系统错误类型
#[derive(Debug, Clone, Copy)]
pub enum FsError {
    /// 无效的文件系统签名
    InvalidSignature,
    /// 块大小不匹配
    BlockSizeMismatch,
    /// 文件未找到
    FileNotFound,
    /// 读取错误
    ReadError,
    /// 内存不足
    OutOfMemory,
    /// 无效路径
    InvalidPath,
    /// 不支持的文件系统
    UnsupportedFs,
}

/// 文件系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemType {
    Fat32,
    ExFat,
    Iso9660,
    Ntfs,  // P2 阶段支持
    Unknown,
}

/// 文件信息
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// 文件名
    pub name: String,
    /// 文件大小 (字节)
    pub size: u64,
    /// 是否为目录
    pub is_dir: bool,
    /// 起始 LBA (用于虚拟 Block IO)
    pub start_lba: u64,
}

/// 文件系统 trait - 所有文件系统必须实现
pub trait FileSystem: Sized {
    /// 文件系统类型
    const FS_TYPE: FileSystemType;

    /// 从 Block IO 初始化文件系统
    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError>;

    /// 读取目录内容
    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError>;

    /// 读取文件内容到缓冲区
    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// 获取文件信息
    fn stat(&self, path: &str) -> Result<FileInfo, FsError>;

    /// 获取块大小
    fn block_size(&self) -> u32;
}

/// Block IO 操作抽象
///
/// 用于解耦文件系统与具体 Block IO 实现
pub trait BlockIoOps {
    /// 获取块大小
    fn block_size(&self) -> u32;

    /// 获取总块数
    fn total_blocks(&self) -> u64;

    /// 读取块
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
}

/// ISO 镜像类型检测
pub fn detect_iso_type(data: &[u8]) -> FileSystemType {
    // ISO9660 检测: 卷描述符位于第 16 个逻辑扇区
    if data.len() >= 0x8000 + 6 {
        let vd = &data[0x8000..];
        if &vd[1..6] == b"CD001" {
            return FileSystemType::Iso9660;
        }
    }

    FileSystemType::Unknown
}

/// 检测文件系统类型
pub fn detect_fs_type(data: &[u8]) -> FileSystemType {
    // FAT32 检测
    if data.len() >= 510 {
        // 检查引导签名
        if data[510] == 0x55 && data[511] == 0xAA {
            // FAT 签名在偏移 0x52 (FAT32) 或 0x03 (FAT12/16)
            if &data[0x52..0x56] == b"FAT32" {
                return FileSystemType::Fat32;
            }
        }
    }

    // exFAT 检测
    if data.len() >= 3 {
        // exFAT 跳转指令和签名
        if data[0] == 0xEB && data[1] == 0x76 && data[2] == 0x90 {
            // 完整签名在偏移 3: "EXFAT"
            if data.len() >= 8 && &data[3..8] == b"EXFAT" {
                return FileSystemType::ExFat;
            }
        }
    }

    FileSystemType::Unknown
}
