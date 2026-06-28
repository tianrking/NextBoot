use alloc::string::String;

/// 文件系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemType {
    Fat32,
    ExFat,
    Iso9660,
    Udf,
    Ext4,
    Xfs,
    Btrfs,
    Ntfs,
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
            FileSystemType::Btrfs => write!(f, "Btrfs"),
            FileSystemType::Ntfs => write!(f, "NTFS"),
            FileSystemType::Unknown => write!(f, "Unknown"),
        }
    }
}

// File attribute flags.
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
