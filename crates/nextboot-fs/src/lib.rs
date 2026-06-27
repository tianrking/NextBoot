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
pub mod fat32;
pub mod gpt;
pub mod iso9660;

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
    Ntfs, // P2 阶段支持
    Unknown,
}

impl core::fmt::Display for FileSystemType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileSystemType::Fat32 => write!(f, "FAT32"),
            FileSystemType::ExFat => write!(f, "exFAT"),
            FileSystemType::Iso9660 => write!(f, "ISO9660"),
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
            // FAT 签名在偏移 0x52 (FAT32) 或 0x03 (FAT12/16)
            if data.len() >= 0x56 && &data[0x52..0x56] == b"FAT32" {
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
mod tests {
    use super::*;
    use crate::exfat::ExFat;
    use crate::fat32::Fat32;
    use crate::iso9660::{get_eltorito_boot_info, read_efi_eltorito_boot_info, Iso9660};
    use alloc::rc::Rc;
    use alloc::vec;

    struct MemoryBlockIo {
        block_size: u32,
        data: Vec<u8>,
    }

    impl MemoryBlockIo {
        fn new(block_size: u32, blocks: usize) -> Self {
            Self {
                block_size,
                data: vec![0; block_size as usize * blocks],
            }
        }

        fn block_mut(&mut self, lba: usize) -> &mut [u8] {
            let block_size = self.block_size as usize;
            let start = lba * block_size;
            &mut self.data[start..start + block_size]
        }
    }

    impl BlockIoOps for MemoryBlockIo {
        fn block_size(&self) -> u32 {
            self.block_size
        }

        fn total_blocks(&self) -> u64 {
            (self.data.len() / self.block_size as usize) as u64
        }

        fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            let block_size = self.block_size as usize;
            let start = lba as usize * block_size;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(FsError::ReadError);
            }
            buf.copy_from_slice(&self.data[start..end]);
            Ok(())
        }
    }

    fn write_iso_record(
        block: &mut [u8],
        offset: usize,
        lba: u32,
        size: u32,
        flags: u8,
        name: &[u8],
    ) {
        let len = 33 + name.len();
        block[offset] = len as u8;
        block[offset + 2..offset + 6].copy_from_slice(&lba.to_le_bytes());
        block[offset + 10..offset + 14].copy_from_slice(&size.to_le_bytes());
        block[offset + 25] = flags;
        block[offset + 28..offset + 30].copy_from_slice(&1u16.to_le_bytes());
        block[offset + 32] = name.len() as u8;
        block[offset + 33..offset + 33 + name.len()].copy_from_slice(name);
    }

    fn write_utf16_name(entry: &mut [u8], name: &str) {
        for (i, ch) in name.encode_utf16().enumerate() {
            let offset = 2 + i * 2;
            entry[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }
    }

    fn write_el_torito_boot_record(block: &mut [u8], catalog_lba: u32) {
        block[0] = 0;
        block[1..6].copy_from_slice(b"CD001");
        block[6] = 1;
        block[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        block[0x47..0x4B].copy_from_slice(&catalog_lba.to_le_bytes());
    }

    fn write_validation_entry(catalog: &mut [u8], platform_id: u8) {
        catalog[0] = 0x01;
        catalog[1] = platform_id;
        catalog[30] = 0x55;
        catalog[31] = 0xAA;
    }

    fn write_boot_entry(
        catalog: &mut [u8],
        offset: usize,
        media_type: u8,
        sector_count: u16,
        image_lba: u32,
    ) {
        catalog[offset] = 0x88;
        catalog[offset + 1] = media_type;
        catalog[offset + 6..offset + 8].copy_from_slice(&sector_count.to_le_bytes());
        catalog[offset + 8..offset + 12].copy_from_slice(&image_lba.to_le_bytes());
    }

    #[test]
    fn read_full_blocks_checks_bounds_and_alignment() {
        let io = MemoryBlockIo::new(512, 2);
        let mut one_block = vec![0u8; 512];
        assert!(read_full_blocks(&io, 0, &mut one_block).is_ok());

        let mut partial = vec![0u8; 128];
        assert!(matches!(
            read_full_blocks(&io, 0, &mut partial),
            Err(FsError::InvalidArgument)
        ));

        let mut too_far = vec![0u8; 512];
        assert!(matches!(
            read_full_blocks(&io, 2, &mut too_far),
            Err(FsError::ReadError)
        ));
    }

    #[test]
    fn iso9660_reads_directory_entries_and_file_data() {
        let mut io = MemoryBlockIo::new(2048, 32);

        {
            let pvd = io.block_mut(16);
            pvd[0] = 1;
            pvd[1..6].copy_from_slice(b"CD001");
            pvd[6] = 1;
            pvd[40..48].copy_from_slice(b"NEXTBOOT");
            pvd[84..88].copy_from_slice(&32u32.to_le_bytes());
            pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
            write_iso_record(pvd, 156, 20, 2048, 0x02, &[0]);
        }

        {
            let end = io.block_mut(17);
            end[0] = 255;
            end[1..6].copy_from_slice(b"CD001");
            end[6] = 1;
        }

        write_iso_record(io.block_mut(20), 0, 21, 11, 0x00, b"KERNEL.;1");
        io.block_mut(21)[..11].copy_from_slice(b"hello world");

        let fs = Iso9660::open(Rc::new(io)).expect("valid ISO9660 filesystem");
        let entries = fs.read_dir("/").expect("root directory");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "kernel");
        assert_eq!(entries[0].size, 11);

        let mut data = [0u8; 11];
        let read = fs.read_file("/kernel", 0, &mut data).expect("file read");
        assert_eq!(read, 11);
        assert_eq!(&data, b"hello world");
    }

    #[test]
    fn eltorito_reads_efi_default_entry() {
        let mut io = MemoryBlockIo::new(2048, 40);
        write_el_torito_boot_record(io.block_mut(17), 22);
        write_validation_entry(io.block_mut(22), 0xEF);
        write_boot_entry(io.block_mut(22), 32, 0, 4, 30);

        let info = read_efi_eltorito_boot_info(&io)
            .expect("read catalog")
            .expect("efi boot entry");

        assert_eq!(info.catalog_lba, 22);
        assert_eq!(info.boot_entry, 0);
        assert_eq!(info.platform_id, 0xEF);
        assert_eq!(info.image_lba, 30);
        assert_eq!(info.image_block_count_2048(), 1);
        assert_eq!(get_eltorito_boot_info(&io.data), Some((30, 4)));
    }

    #[test]
    fn eltorito_prefers_efi_section_entry() {
        let mut io = MemoryBlockIo::new(2048, 64);
        write_el_torito_boot_record(io.block_mut(17), 24);

        {
            let catalog = io.block_mut(24);
            write_validation_entry(catalog, 0x00);
            write_boot_entry(catalog, 32, 0, 4, 31);

            catalog[64] = 0x91;
            catalog[65] = 0xEF;
            catalog[66..68].copy_from_slice(&1u16.to_le_bytes());
            write_boot_entry(catalog, 96, 0, 8, 42);
        }

        let info = read_efi_eltorito_boot_info(&io)
            .expect("read catalog")
            .expect("efi section boot entry");

        assert_eq!(info.boot_entry, 1);
        assert_eq!(info.platform_id, 0xEF);
        assert_eq!(info.image_lba, 42);
        assert_eq!(info.sector_count, 8);
        assert_eq!(info.image_block_count_2048(), 2);
    }

    #[test]
    fn fat32_file_extents_follow_fragmented_cluster_chain() {
        let mut io = MemoryBlockIo::new(512, 16);

        {
            let boot = io.block_mut(0);
            boot[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
            boot[3..11].copy_from_slice(b"NEXTBOOT");
            boot[11..13].copy_from_slice(&512u16.to_le_bytes());
            boot[13] = 1;
            boot[14..16].copy_from_slice(&1u16.to_le_bytes());
            boot[16] = 1;
            boot[32..36].copy_from_slice(&16u32.to_le_bytes());
            boot[36..40].copy_from_slice(&1u32.to_le_bytes());
            boot[44..48].copy_from_slice(&2u32.to_le_bytes());
            boot[82..90].copy_from_slice(b"FAT32   ");
            boot[510] = 0x55;
            boot[511] = 0xAA;
        }

        {
            let fat = io.block_mut(1);
            fat[0..4].copy_from_slice(&0x0FFFFFF8u32.to_le_bytes());
            fat[4..8].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
            fat[8..12].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
            fat[12..16].copy_from_slice(&5u32.to_le_bytes());
            fat[20..24].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
        }

        {
            let root = io.block_mut(2);
            root[0..11].copy_from_slice(b"TEST    ISO");
            root[11] = FileAttributes::ARCHIVE.bits();
            root[26..28].copy_from_slice(&3u16.to_le_bytes());
            root[28..32].copy_from_slice(&700u32.to_le_bytes());
        }

        let fs = Fat32::open(Rc::new(io)).expect("valid FAT32 filesystem");
        let extents = fs.file_extents("/TEST.ISO").expect("file extents");

        assert_eq!(
            extents,
            vec![FileExtent::new(0, 3, 1), FileExtent::new(1, 5, 1),]
        );
    }

    #[test]
    fn exfat_file_extents_support_no_fat_chain_files() {
        let mut io = MemoryBlockIo::new(512, 16);

        {
            let boot = io.block_mut(0);
            boot[0..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
            boot[3..11].copy_from_slice(b"EXFAT   ");
            boot[72..80].copy_from_slice(&16u64.to_le_bytes());
            boot[80..84].copy_from_slice(&1u32.to_le_bytes());
            boot[84..88].copy_from_slice(&1u32.to_le_bytes());
            boot[88..92].copy_from_slice(&2u32.to_le_bytes());
            boot[92..96].copy_from_slice(&14u32.to_le_bytes());
            boot[96..100].copy_from_slice(&2u32.to_le_bytes());
            boot[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
            boot[108] = 9;
            boot[109] = 0;
            boot[110] = 1;
            boot[510..512].copy_from_slice(&0xAA55u16.to_le_bytes());
        }

        {
            let fat = io.block_mut(1);
            fat[8..12].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }

        {
            let root = io.block_mut(2);
            root[0] = 0x85;
            root[1] = 2;
            root[4..6].copy_from_slice(&0x20u16.to_le_bytes());

            root[32] = 0xC0;
            root[33] = 0x02;
            root[35] = 8;
            root[52..56].copy_from_slice(&4u32.to_le_bytes());
            root[56..64].copy_from_slice(&1024u64.to_le_bytes());

            root[64] = 0xC1;
            write_utf16_name(&mut root[64..96], "TEST.ISO");
        }

        io.block_mut(4)[..5].copy_from_slice(b"first");
        io.block_mut(5)[..6].copy_from_slice(b"second");

        let fs = ExFat::open(Rc::new(io)).expect("valid exFAT filesystem");
        let extents = fs.file_extents("/TEST.ISO").expect("file extents");

        assert_eq!(extents, vec![FileExtent::new(0, 4, 2)]);

        let mut data = vec![0u8; 518];
        let read = fs.read_file("/TEST.ISO", 0, &mut data).expect("file read");
        assert_eq!(read, 518);
        assert_eq!(&data[..5], b"first");
        assert_eq!(&data[512..518], b"second");
    }
}
