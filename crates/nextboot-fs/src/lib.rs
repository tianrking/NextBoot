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

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub mod exfat;
pub mod ext4;
pub mod fat32;
pub mod gpt;
pub mod iso9660;
pub mod ntfs;
pub mod udf;
pub mod xfs;

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
    /// 无效参数
    InvalidArgument,
    /// 目录不存在
    DirectoryNotFound,
    /// 不是目录
    NotDirectory,
    /// 不是文件
    NotFile,
    /// 文件太大
    FileTooLarge,
    /// 损坏的文件系统
    Corrupted,
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FsError::InvalidSignature => write!(f, "Invalid filesystem signature"),
            FsError::BlockSizeMismatch => write!(f, "Block size mismatch"),
            FsError::FileNotFound => write!(f, "File not found"),
            FsError::ReadError => write!(f, "Read error"),
            FsError::OutOfMemory => write!(f, "Out of memory"),
            FsError::InvalidPath => write!(f, "Invalid path"),
            FsError::UnsupportedFs => write!(f, "Unsupported filesystem"),
            FsError::InvalidArgument => write!(f, "Invalid argument"),
            FsError::DirectoryNotFound => write!(f, "Directory not found"),
            FsError::NotDirectory => write!(f, "Not a directory"),
            FsError::NotFile => write!(f, "Not a file"),
            FsError::FileTooLarge => write!(f, "File too large"),
            FsError::Corrupted => write!(f, "Corrupted filesystem"),
        }
    }
}

/// 文件系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemType {
    Fat32,
    ExFat,
    Iso9660,
    Udf,
    Ext4,
    Xfs,
    Ntfs, // P2 阶段支持
    Unknown,
}

impl core::fmt::Display for FileSystemType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileSystemType::Fat32 => write!(f, "FAT32"),
            FileSystemType::ExFat => write!(f, "exFAT"),
            FileSystemType::Iso9660 => write!(f, "ISO9660"),
            FileSystemType::Udf => write!(f, "UDF"),
            FileSystemType::Ext4 => write!(f, "ext4"),
            FileSystemType::Xfs => write!(f, "XFS"),
            FileSystemType::Ntfs => write!(f, "NTFS"),
            FileSystemType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// 文件属性标志
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct FileAttributes: u8 {
        const READ_ONLY = 0x01;
        const HIDDEN = 0x02;
        const SYSTEM = 0x04;
        const VOLUME_ID = 0x08;
        const DIRECTORY = 0x10;
        const ARCHIVE = 0x20;
    }
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
    /// 文件属性
    pub attributes: FileAttributes,
    /// 起始簇号 (FAT) 或 LBA (ISO9660)
    pub start_cluster: u64,
    /// 文件数据是否按起始簇连续分配
    pub contiguous: bool,
}

impl FileInfo {
    /// 创建新的文件信息
    pub fn new(name: String, size: u64, is_dir: bool, start_cluster: u64) -> Self {
        Self {
            name,
            size,
            is_dir,
            attributes: if is_dir {
                FileAttributes::DIRECTORY
            } else {
                FileAttributes::empty()
            },
            start_cluster,
            contiguous: false,
        }
    }

    /// 检查是否为隐藏文件
    pub fn is_hidden(&self) -> bool {
        self.attributes.contains(FileAttributes::HIDDEN)
    }

    /// 检查是否为系统文件
    pub fn is_system(&self) -> bool {
        self.attributes.contains(FileAttributes::SYSTEM)
    }
}

/// 文件在底层块设备上的物理区段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileExtent {
    /// 文件内的虚拟块起始位置
    pub virtual_block_start: u64,
    /// 底层块设备上的物理 LBA
    pub physical_lba: u64,
    /// 连续块数量
    pub block_count: u64,
}

impl FileExtent {
    pub fn new(virtual_block_start: u64, physical_lba: u64, block_count: u64) -> Self {
        Self {
            virtual_block_start,
            physical_lba,
            block_count,
        }
    }

    pub fn virtual_block_end(&self) -> u64 {
        self.virtual_block_start + self.block_count
    }

    pub fn physical_lba_end(&self) -> u64 {
        self.physical_lba + self.block_count
    }
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

/// Shared block device handle used by filesystem instances.
pub type SharedBlockIo = Rc<dyn BlockIoOps>;

impl<T: BlockIoOps + ?Sized> BlockIoOps for Rc<T> {
    fn block_size(&self) -> u32 {
        (**self).block_size()
    }

    fn total_blocks(&self) -> u64 {
        (**self).total_blocks()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        (**self).read_blocks(lba, buf)
    }
}

/// Validate and read one or more full hardware blocks.
pub fn read_full_blocks(
    block_io: &dyn BlockIoOps,
    lba: u64,
    buf: &mut [u8],
) -> Result<(), FsError> {
    let block_size = block_io.block_size() as usize;
    if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
        return Err(FsError::InvalidArgument);
    }

    let block_count = (buf.len() / block_size) as u64;
    if lba
        .checked_add(block_count)
        .map_or(true, |end| end > block_io.total_blocks())
    {
        return Err(FsError::ReadError);
    }

    block_io.read_blocks(lba, buf)
}

/// 动态分发的 Block IO
pub struct DynBlockIo {
    block_size: u32,
    total_blocks: u64,
    read_fn: fn(u64, &mut [u8]) -> Result<(), FsError>,
}

impl DynBlockIo {
    pub fn new(
        block_size: u32,
        total_blocks: u64,
        read_fn: fn(u64, &mut [u8]) -> Result<(), FsError>,
    ) -> Self {
        Self {
            block_size,
            total_blocks,
            read_fn,
        }
    }
}

impl BlockIoOps for DynBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        (self.read_fn)(lba, buf)
    }
}

/// 文件系统 trait - 所有文件系统必须实现
pub trait FileSystem: Sized {
    /// 文件系统类型
    const FS_TYPE: FileSystemType;

    /// 从 Block IO 初始化文件系统
    fn init(block_io: SharedBlockIo) -> Result<Self, FsError>;

    /// 读取目录内容
    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError>;

    /// 读取文件内容到缓冲区
    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// 获取文件信息
    fn stat(&self, path: &str) -> Result<FileInfo, FsError>;

    /// 获取块大小
    fn block_size(&self) -> u32;

    /// 获取文件到底层块设备的物理 LBA 映射。
    fn file_extents(&self, _path: &str) -> Result<Vec<FileExtent>, FsError> {
        Err(FsError::UnsupportedFs)
    }

    /// 递归扫描目录获取所有文件
    fn scan_files(&self, path: &str, extensions: &[&str]) -> Result<Vec<FileInfo>, FsError> {
        let mut result = Vec::new();
        self.scan_files_recursive(path, extensions, &mut result)?;
        Ok(result)
    }

    /// 递归扫描辅助函数
    fn scan_files_recursive(
        &self,
        path: &str,
        extensions: &[&str],
        result: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let entries = self.read_dir(path)?;

        for entry in entries {
            // 跳过隐藏和系统文件
            if entry.is_hidden() || entry.is_system() {
                continue;
            }

            let full_path = if path == "/" || path.is_empty() {
                alloc::format!("/{}", entry.name)
            } else {
                alloc::format!("{}/{}", path, entry.name)
            };

            if entry.is_dir {
                // 递归扫描子目录
                self.scan_files_recursive(&full_path, extensions, result)?;
            } else {
                // 检查扩展名
                let name_lower = entry.name.to_ascii_lowercase();
                let matches =
                    extensions.is_empty() || extensions.iter().any(|ext| name_lower.ends_with(ext));

                if matches {
                    let mut file_info = entry.clone();
                    file_info.name = full_path;
                    result.push(file_info);
                }
            }
        }

        Ok(())
    }
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
            // FAT32 filesystem type is an 8-byte field at offset 0x52.
            if data.len() >= 0x5A && data[0x52..0x5A].starts_with(b"FAT32") {
                return FileSystemType::Fat32;
            }
            // FAT12/16 签名
            if data.len() >= 0x08 && &data[0x03..0x08] == b"FAT12" {
                return FileSystemType::Fat32; // 简化处理
            }
            if data.len() >= 0x08 && &data[0x03..0x08] == b"FAT16" {
                return FileSystemType::Fat32; // 简化处理
            }
        }
    }

    // exFAT 检测
    if data.len() >= 3 {
        // exFAT 跳转指令和签名
        if data[0] == 0xEB && data[1] == 0x76 && data[2] == 0x90 {
            // 完整签名在偏移 3: "EXFAT"
            if data.len() >= 11 && &data[3..11] == b"EXFAT   " {
                return FileSystemType::ExFat;
            }
        }
    }

    // NTFS 检测
    if data.len() >= 11 && &data[3..11] == b"NTFS    " {
        return FileSystemType::Ntfs;
    }

    // XFS 检测
    if data.len() >= 4 && &data[0..4] == b"XFSB" {
        return FileSystemType::Xfs;
    }

    // ISO9660 检测
    detect_iso_type(data)
}

/// 路径规范化
pub fn normalize_path(path: &str) -> String {
    let mut result = String::new();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    for part in parts {
        if part == "." {
            continue;
        }
        if part == ".." {
            // 简化处理，不支持 ..
            continue;
        }
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }

    if result.is_empty() {
        String::from("/")
    } else {
        result
    }
}

/// 分割路径为目录和文件名
pub fn split_path(path: &str) -> (String, String) {
    let normalized = normalize_path(path);
    if let Some(pos) = normalized.rfind('/') {
        let dir = &normalized[..pos];
        let name = &normalized[pos + 1..];
        (
            if dir.is_empty() {
                String::from("/")
            } else {
                dir.to_string()
            },
            name.to_string(),
        )
    } else {
        (String::from("/"), normalized)
    }
}

/// 全局分配器辅助函数
pub fn alloc_buffer(size: usize) -> Result<Vec<u8>, FsError> {
    let mut buf = Vec::new();
    buf.try_reserve(size).map_err(|_| FsError::OutOfMemory)?;
    buf.resize(size, 0);
    Ok(buf)
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
