use crate::source_disk::{PartitionFormat, SourceDiskIdentity};
use crate::ventoy_config::{VentoyImagePlugin, VentoyMenuTip, VentoyPassword};
use crate::wim;
use alloc::string::String;
use alloc::vec::Vec;
use nextboot_fs::iso9660::ElToritoBootInfo;
use nextboot_fs::FileExtent;
use uefi::Handle;

/// ISO 文件信息
#[derive(Debug, Clone)]
pub struct IsoFile {
    /// 文件路径
    pub path: String,
    /// Ventoy menu_alias 插件提供的显示名。
    pub menu_alias: Option<String>,
    /// Ventoy menu_class 插件为该镜像匹配出的菜单 class。
    pub ventoy_menu_class: Option<String>,
    /// Ventoy menu_tip 插件为该镜像匹配出的提示。
    pub ventoy_menu_tip: Option<VentoyMenuTip>,
    /// Ventoy control.VTOY_DEFAULT_IMAGE 是否指向该镜像。
    pub ventoy_default_image: bool,
    /// Ventoy control.VTOY_MENU_TIMEOUT 为该卷菜单设置的自动启动超时。
    pub ventoy_menu_timeout: Option<u32>,
    /// Ventoy control.VTOY_LINUX_REMOUNT 是否要求 Linux hook 重新挂载源盘。
    pub ventoy_linux_remount: bool,
    /// Ventoy control.VTOY_WINDOWS_CD_PROMPT 是否保留 Windows CD/DVD 提示。
    pub ventoy_windows_cd_prompt: bool,
    /// Ventoy control.VTOY_WIN_UEFI_RES_LOCK 映射到 Windows UEFI 分辨率锁定模式。
    pub ventoy_windows_uefi_resolution_lock: u8,
    /// Ventoy control.VTOY_WIN11_BYPASS_CHECK 是否要求跳过 Win11 安装硬件检查。
    pub ventoy_windows11_bypass_check: bool,
    /// Ventoy control.VTOY_WIN11_BYPASS_NRO 是否要求跳过 Win11 OOBE 联网账号要求。
    pub ventoy_windows11_bypass_nro: bool,
    /// Ventoy password 插件为该镜像匹配出的密码。
    pub ventoy_password: Option<VentoyPassword>,
    /// Ventoy password.bootpwd 为该卷设置的全局启动菜单密码。
    pub ventoy_boot_password: Option<VentoyPassword>,
    /// Ventoy 启动相关插件为该镜像匹配出的配置。
    pub ventoy_plugin: Option<VentoyImagePlugin>,
    /// 文件大小 (字节)
    pub size: u64,
    /// 启动时呈现给固件/系统的虚拟介质大小
    pub virtual_size: u64,
    /// 启动时呈现给固件/系统的虚拟逻辑块大小
    pub virtual_block_size: Option<u32>,
    /// 文件所在的 UEFI volume handle
    pub volume_handle: Handle,
    /// Ventoy 插件和运行时资产所在的卷；普通镜像与 volume_handle 相同，VLNK 为指针文件所在卷。
    pub asset_volume_handle: Handle,
    /// 扫描时分配的卷索引，用于区分不同卷上的同名镜像
    pub volume_index: usize,
    /// 文件所在卷的逻辑块大小
    pub block_size: u32,
    /// 起始 LBA
    pub start_lba: u64,
    /// 文件到底层卷 BlockIO 的 extent 映射
    pub extents: Vec<IsoExtent>,
    /// 检测到的操作系统类型
    pub os_type: OsType,
    /// 镜像格式
    pub image_format: ImageFormat,
    /// ISO 内的 EFI El Torito 启动镜像信息
    pub boot_info: Option<IsoBootInfo>,
    /// ISO 是否包含 Ventoy 兼容的 UDF volume recognition sequence。
    pub is_udf: bool,
    /// WIM/ESD 启动元数据。
    pub wim_info: Option<WimBootInfo>,
    /// 文件所在源盘/分区身份，用于 Ventoy 兼容 OS 参数。
    pub source_disk: Option<SourceDiskIdentity>,
    /// Ventoy 插件和运行时资产所在源盘/分区身份。
    pub asset_source_disk: Option<SourceDiskIdentity>,
    /// 源盘身份不可用时用于 Ventoy OS 参数的源卷/源盘容量兜底。
    pub source_disk_size: u64,
    /// 该菜单项是否来自 Ventoy `.vlnk.*` 指针文件。
    pub is_vlnk: bool,
    /// VLNK 指向的真实镜像路径。
    pub vlnk_target_path: Option<String>,
}

/// ISO 文件在所在卷上的物理区段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoExtent {
    pub virtual_block_start: u64,
    pub physical_lba: u64,
    pub block_count: u64,
}

/// ISO 内 El Torito EFI 启动镜像信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoBootInfo {
    pub catalog_lba: u32,
    pub boot_entry: u32,
    pub platform_id: u8,
    pub image_lba: u32,
    pub image_block_count: u64,
    pub sector_count: u16,
}

impl IsoBootInfo {
    pub fn is_efi(&self) -> bool {
        self.platform_id == 0xEF
    }
}

impl From<ElToritoBootInfo> for IsoBootInfo {
    fn from(info: ElToritoBootInfo) -> Self {
        Self {
            catalog_lba: info.catalog_lba,
            boot_entry: info.boot_entry,
            platform_id: info.platform_id,
            image_lba: info.image_lba,
            image_block_count: info.image_block_count_2048(),
            sector_count: info.sector_count,
        }
    }
}

/// WIM/ESD 启动元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimBootInfo {
    pub header_len: u32,
    pub version: u32,
    pub flags: u32,
    pub compression: WimCompression,
    pub chunk_len: u32,
    pub image_count: u32,
    pub boot_index: u32,
    pub boot_index_in_range: bool,
    pub wimboot_supported: bool,
}

impl WimBootInfo {
    pub fn is_bootable(&self) -> bool {
        self.boot_index != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimCompression {
    None,
    Xpress,
    Lzx,
    Lzms,
}

impl From<wim::WimCompression> for WimCompression {
    fn from(compression: wim::WimCompression) -> Self {
        match compression {
            wim::WimCompression::None => Self::None,
            wim::WimCompression::Xpress => Self::Xpress,
            wim::WimCompression::Lzx => Self::Lzx,
            wim::WimCompression::Lzms => Self::Lzms,
        }
    }
}

impl From<wim::WimMetadata> for WimBootInfo {
    fn from(metadata: wim::WimMetadata) -> Self {
        Self {
            header_len: metadata.header_len,
            version: metadata.version,
            flags: metadata.flags,
            compression: metadata.compression.into(),
            chunk_len: metadata.chunk_len,
            image_count: metadata.image_count,
            boot_index: metadata.boot_index,
            boot_index_in_range: metadata.boot_index_in_range(),
            wimboot_supported: metadata.is_wimboot_supported(),
        }
    }
}

impl From<FileExtent> for IsoExtent {
    fn from(extent: FileExtent) -> Self {
        Self {
            virtual_block_start: extent.virtual_block_start,
            physical_lba: extent.physical_lba,
            block_count: extent.block_count,
        }
    }
}

pub(super) struct ResolvedImageMetadata {
    pub(super) block_size: u32,
    pub(super) extents: Vec<IsoExtent>,
    pub(super) boot_info: Option<IsoBootInfo>,
    pub(super) is_udf: bool,
    pub(super) wim_info: Option<WimBootInfo>,
    pub(super) image_format: ImageFormat,
    pub(super) virtual_size: u64,
    pub(super) virtual_block_size: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct VolumeBlockInfo {
    pub(super) block_size: u32,
    pub(super) total_size: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PartitionCandidate {
    pub(super) number: u32,
    pub(super) start_lba: u64,
    pub(super) block_count: u64,
    pub(super) format: PartitionFormat,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PartitionRange {
    pub(super) start_lba: u64,
    pub(super) block_count: u64,
}

impl PartitionRange {
    pub(super) fn matches(self, start_lba: u64, block_count: u64) -> bool {
        self.start_lba == start_lba && self.block_count == block_count
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MbrPartitionEntry {
    pub(super) partition_type: u8,
    pub(super) start_lba: u32,
    pub(super) total_sectors: u32,
}

/// 可启动镜像格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Iso,
    Wim,
    Esd,
    EfiExecutable,
    RawDisk,
    Vhd,
    FixedVhd,
    DynamicVhd,
    DifferencingVhd,
    Vhdx,
    Vdi,
    Unknown,
}

impl ImageFormat {
    pub fn detect_from_path(path: &str) -> Self {
        let lower = path.to_lowercase();
        if lower.ends_with(".iso") {
            Self::Iso
        } else if lower.ends_with(".wim") {
            Self::Wim
        } else if lower.ends_with(".esd") {
            Self::Esd
        } else if lower.ends_with(".efi") {
            Self::EfiExecutable
        } else if lower.ends_with(".img") {
            Self::RawDisk
        } else if lower.ends_with(".vhd") {
            Self::Vhd
        } else if lower.ends_with(".vhdx") {
            Self::Vhdx
        } else if lower.ends_with(".vdi") {
            Self::Vdi
        } else {
            Self::Unknown
        }
    }

    pub fn from_vhd_disk_type(disk_type: u32) -> Self {
        match disk_type {
            2 => Self::FixedVhd,
            3 => Self::DynamicVhd,
            4 => Self::DifferencingVhd,
            _ => Self::Vhd,
        }
    }

    pub fn is_iso(self) -> bool {
        self == Self::Iso
    }

    pub fn is_efi_executable(self) -> bool {
        self == Self::EfiExecutable
    }

    pub fn is_wim_container(self) -> bool {
        matches!(self, Self::Wim | Self::Esd)
    }

    pub fn supports_virtual_disk_boot(self) -> bool {
        matches!(
            self,
            Self::Iso | Self::RawDisk | Self::FixedVhd | Self::DynamicVhd | Self::Vhdx | Self::Vdi
        )
    }

    pub fn uses_512_byte_virtual_sectors(self) -> bool {
        matches!(
            self,
            Self::RawDisk | Self::FixedVhd | Self::DynamicVhd | Self::Vdi
        )
    }
}

impl core::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self {
            ImageFormat::Iso => "ISO",
            ImageFormat::Wim => "WIM",
            ImageFormat::Esd => "ESD",
            ImageFormat::EfiExecutable => "EFI",
            ImageFormat::RawDisk => "RAW",
            ImageFormat::Vhd => "VHD",
            ImageFormat::FixedVhd => "Fixed VHD",
            ImageFormat::DynamicVhd => "Dynamic VHD",
            ImageFormat::DifferencingVhd => "Differencing VHD",
            ImageFormat::Vhdx => "VHDX",
            ImageFormat::Vdi => "VDI",
            ImageFormat::Unknown => "Unknown",
        };
        write!(f, "{}", name)
    }
}

/// 操作系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Windows,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    Linux,
    WinPE,
    Unknown,
}

impl OsType {
    /// 从文件名检测
    pub fn detect_from_path(path: &str) -> Self {
        let path_lower = path.to_lowercase();

        if path_lower.contains("windows") {
            return OsType::Windows;
        }
        if path_lower.contains("ubuntu") {
            return OsType::Ubuntu;
        }
        if path_lower.contains("debian") {
            return OsType::Debian;
        }
        if path_lower.contains("fedora") {
            return OsType::Fedora;
        }
        if path_lower.contains("arch") || path_lower.contains("manjaro") {
            return OsType::Arch;
        }
        if path_lower.contains("winpe") || path_lower.contains("pe_") {
            return OsType::WinPE;
        }
        if path_lower.contains("linux") {
            return OsType::Linux;
        }

        OsType::Unknown
    }
}

pub struct IsoCache {
    entries: Vec<IsoFile>,
    timestamp: u64,
}

impl IsoCache {
    /// 创建新缓存
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            timestamp: 0,
        }
    }

    /// 从缓存加载
    pub fn load(&self) -> Option<&[IsoFile]> {
        if self.entries.is_empty() {
            None
        } else {
            Some(&self.entries)
        }
    }

    /// 保存到缓存
    pub fn save(&mut self, entries: Vec<IsoFile>) {
        self.entries = entries;
        // timestamp = current_time
    }

    /// 清除缓存
    pub fn clear(&mut self) {
        self.entries.clear();
        self.timestamp = 0;
    }

    /// 检查缓存是否有效
    pub fn is_valid(&self, _max_age_seconds: u64) -> bool {
        // TODO: 检查时间戳
        !self.entries.is_empty()
    }
}

impl Default for IsoCache {
    fn default() -> Self {
        Self::new()
    }
}
