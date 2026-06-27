use bitflags::bitflags;

/// 虚拟设备类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualDeviceType {
    /// 模拟 DVD 光驱
    DvdRom,
    /// 模拟硬盘
    HardDisk,
    /// 模拟 USB 存储设备
    UsbMassStorage,
}

impl VirtualDeviceType {
    /// 获取设备类型描述
    pub fn description(&self) -> &'static str {
        match self {
            VirtualDeviceType::DvdRom => "Virtual DVD-ROM",
            VirtualDeviceType::HardDisk => "Virtual Hard Disk",
            VirtualDeviceType::UsbMassStorage => "Virtual USB Storage",
        }
    }

    /// 获取 UEFI 媒体类型
    pub fn uefi_media_type(&self) -> u8 {
        match self {
            VirtualDeviceType::DvdRom => 0x02,         // CD-ROM
            VirtualDeviceType::HardDisk => 0x01,       // Hard Disk
            VirtualDeviceType::UsbMassStorage => 0x01, // Hard Disk
        }
    }
}

/// El Torito CD-ROM boot image location used in UEFI device paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdRomBootInfo {
    pub boot_entry: u32,
    pub image_lba: u64,
    pub image_block_count: u64,
}

impl CdRomBootInfo {
    pub fn new(boot_entry: u32, image_lba: u64, image_block_count: u64) -> Self {
        Self {
            boot_entry,
            image_lba,
            image_block_count: image_block_count.max(1),
        }
    }
}

/// 虚拟设备配置
#[derive(Debug, Clone)]
pub struct VirtualDeviceConfig {
    /// 设备类型
    pub device_type: VirtualDeviceType,
    /// ISO 文件起始 LBA
    pub iso_start_lba: u64,
    /// ISO 文件大小 (字节)
    pub iso_size: u64,
    /// 块大小
    pub block_size: u32,
    /// 底层物理块大小
    pub physical_block_size: u32,
    /// 设备名称
    pub device_name: alloc::string::String,
    /// Optional El Torito boot image used by virtual CD-ROM device paths.
    pub cdrom_boot: Option<CdRomBootInfo>,
}

impl VirtualDeviceConfig {
    /// 创建新的虚拟设备配置
    pub fn new(
        device_type: VirtualDeviceType,
        iso_start_lba: u64,
        iso_size: u64,
        block_size: u32,
    ) -> Self {
        Self {
            device_type,
            iso_start_lba,
            iso_size,
            block_size,
            physical_block_size: block_size,
            device_name: alloc::string::String::from("NextBoot Virtual Device"),
            cdrom_boot: None,
        }
    }

    /// 计算 ISO 文件占用的块数
    pub fn block_count(&self) -> u64 {
        (self.iso_size + self.block_size as u64 - 1) / self.block_size as u64
    }

    /// 设置设备名称
    pub fn with_name(mut self, name: &str) -> Self {
        self.device_name = alloc::string::String::from(name);
        self
    }

    /// 设置底层物理块大小。
    pub fn with_physical_block_size(mut self, physical_block_size: u32) -> Self {
        self.physical_block_size = physical_block_size;
        self
    }

    /// Set the El Torito boot image for virtual CD-ROM media.
    pub fn with_cdrom_boot(mut self, boot: CdRomBootInfo) -> Self {
        self.cdrom_boot = Some(boot);
        self
    }
}

/// 虚拟设备信息
#[derive(Debug, Clone)]
pub struct VirtualDeviceInfo {
    /// 设备类型
    pub device_type: VirtualDeviceType,
    /// 块大小
    pub block_size: u32,
    /// 块数量
    pub block_count: u64,
    /// 总大小 (字节)
    pub size_bytes: u64,
    /// 是否只读
    pub read_only: bool,
    /// 媒体是否存在
    pub media_present: bool,
    /// 媒体 ID
    pub media_id: u32,
}

/// 虚拟 IO 错误
#[derive(Debug, Clone, Copy)]
pub enum VirtIoError {
    /// 超出边界
    OutOfBounds,
    /// 写保护
    WriteProtected,
    /// 读取失败
    ReadFailed,
    /// 无效参数
    InvalidArgument,
    /// 无效缓冲区大小
    InvalidBufferSize,
    /// 媒体已更改
    MediaChanged,
    /// 无效的 LBA 映射
    InvalidMapping,
    /// 未设置物理读取函数
    NoPhysicalRead,
    /// 设备错误
    DeviceError,
    /// CRC 错误
    CrcError,
}

impl core::fmt::Display for VirtIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VirtIoError::OutOfBounds => write!(f, "LBA out of bounds"),
            VirtIoError::WriteProtected => write!(f, "Device is write protected"),
            VirtIoError::ReadFailed => write!(f, "Read operation failed"),
            VirtIoError::InvalidArgument => write!(f, "Invalid argument"),
            VirtIoError::InvalidBufferSize => {
                write!(f, "Buffer size must be multiple of block size")
            }
            VirtIoError::MediaChanged => write!(f, "Media changed"),
            VirtIoError::InvalidMapping => write!(f, "Invalid LBA mapping"),
            VirtIoError::NoPhysicalRead => write!(f, "No physical read function set"),
            VirtIoError::DeviceError => write!(f, "Device error"),
            VirtIoError::CrcError => write!(f, "CRC error"),
        }
    }
}

bitflags! {
    /// UEFI Block IO 媒体标志
    #[derive(Debug, Clone, Copy)]
    pub struct MediaFlags: u32 {
        /// 媒体是否存在
        const MEDIA_PRESENT = 1 << 0;
        /// 是否为只读
        const READ_ONLY = 1 << 1;
        /// 是否为可移动设备
        const REMOVABLE = 1 << 2;
        /// 是否使用 4K 扇区
        const USE_4K = 1 << 3;
        /// 是否为逻辑分区
        const LOGICAL_PARTITION = 1 << 4;
        /// 是否启用写入缓存
        const WRITE_CACHING = 1 << 5;
    }
}

impl Default for MediaFlags {
    fn default() -> Self {
        MediaFlags::MEDIA_PRESENT | MediaFlags::READ_ONLY
    }
}

/// 虚拟设备媒体信息
#[derive(Debug, Clone)]
pub struct VirtualMediaInfo {
    /// 块大小
    pub block_size: u32,
    /// 最后一个块 LBA
    pub last_block: u64,
    /// 媒体标志
    pub flags: MediaFlags,
    /// 设备类型字符串
    pub device_type_str: &'static str,
    /// 媒体 ID
    pub media_id: u32,
}

impl VirtualMediaInfo {
    /// 从配置创建媒体信息
    pub fn from_config(config: &VirtualDeviceConfig) -> Self {
        let flags = MediaFlags::MEDIA_PRESENT
            | MediaFlags::READ_ONLY
            | MediaFlags::REMOVABLE
            | if config.block_size == 4096 {
                MediaFlags::USE_4K
            } else {
                MediaFlags::empty()
            };

        Self {
            block_size: config.block_size,
            last_block: config.block_count() - 1,
            flags,
            device_type_str: config.device_type.description(),
            media_id: 0x4E425453,
        }
    }

    /// 获取总大小 (字节)
    pub fn total_size(&self) -> u64 {
        (self.last_block + 1) * self.block_size as u64
    }
}

/// ISO 文件映射信息
#[derive(Debug, Clone)]
pub struct IsoMapping {
    /// ISO 文件名
    pub filename: alloc::string::String,
    /// ISO 文件大小
    pub size: u64,
    /// 起始 LBA
    pub start_lba: u64,
    /// 块数量
    pub block_count: u64,
    /// 块大小
    pub block_size: u32,
    /// 设备类型
    pub device_type: VirtualDeviceType,
}

impl IsoMapping {
    /// 创建新的 ISO 映射
    pub fn new(filename: &str, size: u64, start_lba: u64, block_size: u32) -> Self {
        let block_count = (size + block_size as u64 - 1) / block_size as u64;

        // 根据 ISO 类型决定设备类型
        let device_type = if filename.to_lowercase().contains("windows") {
            VirtualDeviceType::DvdRom
        } else {
            VirtualDeviceType::HardDisk
        };

        Self {
            filename: alloc::string::String::from(filename),
            size,
            start_lba,
            block_count,
            block_size,
            device_type,
        }
    }

    /// 转换为虚拟设备配置
    pub fn to_config(&self) -> VirtualDeviceConfig {
        VirtualDeviceConfig::new(self.device_type, self.start_lba, self.size, self.block_size)
            .with_name(&self.filename)
    }
}
