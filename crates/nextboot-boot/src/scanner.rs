//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use crate::source_disk::{
    build_source_disk_identity, parent_device_path_bytes, parse_last_hard_drive_device_path,
    HardDriveDevicePathInfo, PartitionFormat, SourceDiskIdentity,
};
use crate::vdi;
use crate::ventoy_config::{
    VentoyConfig, VentoyConfigError, VentoyImagePlugin, VentoyMenuTip, VentoyPassword,
};
use crate::vhdx;
use crate::vlnk::{self, VentoyVlnk};
use crate::wim;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr::{self, NonNull};
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::{
    detect_udf_volume, read_efi_eltorito_boot_info, ElToritoBootInfo, Iso9660,
};
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileExtent, FileSystem, FileSystemType, FsError};
use nextboot_virtio::{
    PhysicalReader, VirtIoError, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType,
};
use uefi::data_types::CString16;
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::{Handle, Identify, Status};

const VENTOY_CONFIG_PATH: &str = "/ventoy/ventoy.json";
const VENTOY_CONFIG_MAX_SIZE: usize = 256 * 1024;
const MBR_PARTITION_TABLE_OFFSET: usize = 0x1be;
const MBR_PARTITION_ENTRY_SIZE: usize = 16;
const MBR_PRIMARY_PARTITION_COUNT: usize = 4;
const MBR_LOGICAL_PARTITION_NUMBER_BASE: u32 = 5;
const MBR_MAX_LOGICAL_PARTITIONS: usize = 128;
const GPT_HEADER_LBA: u64 = 1;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_HEADER_MIN_SIZE: u32 = 92;
const GPT_PARTITION_ENTRY_LBA_OFFSET: usize = 72;
const GPT_NUM_PARTITION_ENTRIES_OFFSET: usize = 80;
const GPT_PARTITION_ENTRY_SIZE_OFFSET: usize = 84;
const GPT_MIN_PARTITION_ENTRY_SIZE: usize = 128;
const GPT_MAX_PARTITION_ENTRY_SIZE: usize = 4096;
const GPT_MAX_PARTITION_ENTRIES: usize = 4096;
const GPT_MAX_PARTITION_ENTRY_ARRAY_BYTES: usize = 1024 * 1024;

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

struct ResolvedImageMetadata {
    block_size: u32,
    extents: Vec<IsoExtent>,
    boot_info: Option<IsoBootInfo>,
    is_udf: bool,
    wim_info: Option<WimBootInfo>,
    image_format: ImageFormat,
    virtual_size: u64,
    virtual_block_size: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct VolumeBlockInfo {
    block_size: u32,
    total_size: u64,
}

#[derive(Debug, Clone, Copy)]
struct PartitionCandidate {
    number: u32,
    start_lba: u64,
    block_count: u64,
    format: PartitionFormat,
}

#[derive(Debug, Clone, Copy)]
struct PartitionRange {
    start_lba: u64,
    block_count: u64,
}

impl PartitionRange {
    fn matches(self, start_lba: u64, block_count: u64) -> bool {
        self.start_lba == start_lba && self.block_count == block_count
    }
}

#[derive(Debug, Clone, Copy)]
struct MbrPartitionEntry {
    partition_type: u8,
    start_lba: u32,
    total_sectors: u32,
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

/// ISO 扫描器
pub struct IsoScanner<'a> {
    bt: &'a BootServices,
}

impl<'a> IsoScanner<'a> {
    /// 创建新的扫描器
    pub fn new(bt: &'a BootServices) -> Self {
        Self { bt }
    }

    /// 扫描指定目录下的 ISO 文件
    pub fn scan(&self, root: &str) -> uefi::Result<Vec<IsoFile>> {
        let mut iso_files = Vec::new();

        // 支持的文件扩展名
        let extensions = [
            ".iso",
            ".wim",
            ".img",
            ".vhd",
            ".vhdx",
            ".vdi",
            ".esd",
            ".efi",
            ".vlnk.dat",
            ".vlnk.vtoy",
        ];

        // 扫描常见目录
        let default_search_paths = [
            root, "/", "/ISO", "/iso", "/Images", "/images", "/Boot", "/boot",
        ];

        let simple_fs_handles: Vec<Handle> = match self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        {
            Ok(handles) => handles.iter().copied().collect(),
            Err(err) if err.status() == Status::NOT_FOUND => {
                log::warn!("No SimpleFileSystem handles found; falling back to raw BlockIO scan");
                Vec::new()
            }
            Err(err) => return Err(err),
        };

        for (volume_index, handle) in simple_fs_handles.iter().copied().enumerate() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };
            let config = self.load_ventoy_config(&mut fs);
            let search_paths = config.search_roots(&default_search_paths);

            for search_path in &search_paths {
                if let Ok(files) = self.scan_volume_path(
                    volume_index,
                    handle,
                    &mut fs,
                    search_path,
                    &extensions,
                    &config,
                ) {
                    iso_files.extend(files);
                }
            }
        }

        if let Ok(mut block_files) = self.scan_block_filesystem_volumes(
            simple_fs_handles.len(),
            &simple_fs_handles,
            &default_search_paths,
            &extensions,
        ) {
            iso_files.append(&mut block_files);
        }

        // 去重。相同卷上的相同路径可能会被多个 search path 扫到；FAT/exFAT/NTFS
        // 路径大小写不敏感，所以 /ISO 与 /iso 命中同一个文件时也要合并。
        // 不同卷上的同名镜像必须保留，这是固定盘/多 SSD 场景的关键差异。
        iso_files.sort_by(|a, b| {
            a.volume_index
                .cmp(&b.volume_index)
                .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
        iso_files.dedup_by(|a, b| {
            a.volume_index == b.volume_index && a.path.eq_ignore_ascii_case(&b.path)
        });

        // 按名称排序
        iso_files.sort_by(|a, b| {
            a.path
                .split('/')
                .last()
                .unwrap_or(&a.path)
                .cmp(b.path.split('/').last().unwrap_or(&b.path))
                .then_with(|| a.volume_index.cmp(&b.volume_index))
                .then_with(|| a.path.cmp(&b.path))
        });

        Ok(iso_files)
    }

    /// 扫描单个目录
    fn scan_directory(&self, path: &str, extensions: &[&str]) -> uefi::Result<Vec<IsoFile>> {
        let simple_fs_handles: Vec<Handle> = match self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        {
            Ok(handles) => handles.iter().copied().collect(),
            Err(err) if err.status() == Status::NOT_FOUND => {
                log::warn!("No SimpleFileSystem handles found; falling back to raw BlockIO scan");
                Vec::new()
            }
            Err(err) => return Err(err),
        };
        let mut files = Vec::new();

        for (volume_index, handle) in simple_fs_handles.iter().copied().enumerate() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };
            let config = self.load_ventoy_config(&mut fs);

            if let Ok(mut volume_files) =
                self.scan_volume_path(volume_index, handle, &mut fs, path, extensions, &config)
            {
                files.append(&mut volume_files);
            }
        }

        if let Ok(mut block_files) = self.scan_block_filesystem_volumes(
            simple_fs_handles.len(),
            &simple_fs_handles,
            &[path],
            extensions,
        ) {
            files.append(&mut block_files);
        }

        Ok(files)
    }

    /// 检测 ISO 文件类型
    fn detect_iso_type(&self, path: &str) -> OsType {
        OsType::detect_from_path(path)
    }

    fn detect_image_os_type(
        &self,
        path: &str,
        image_format: ImageFormat,
        wim_info: Option<WimBootInfo>,
    ) -> OsType {
        if image_format.is_wim_container() {
            if let Some(info) = wim_info {
                if info.is_bootable() {
                    return OsType::WinPE;
                }
            }
        }

        self.detect_iso_type(path)
    }

    fn scan_volume_path(
        &self,
        volume_index: usize,
        volume_handle: Handle,
        fs: &mut SimpleFileSystem,
        path: &str,
        extensions: &[&str],
        config: &VentoyConfig,
    ) -> uefi::Result<Vec<IsoFile>> {
        let mut root = fs.open_volume()?;
        let normalized = normalize_scan_path(path);
        if is_ventoy_plugin_tree_path(&normalized) {
            return Ok(Vec::new());
        }
        let mut dir = if normalized == "/" {
            root
        } else {
            match open_directory(&mut root, &normalized) {
                Ok(dir) => dir,
                Err(e) => return Err(e),
            }
        };

        let mut files = Vec::new();
        let source_disk = self.resolve_source_disk_identity(volume_handle);
        let volume_info = self.volume_block_info(volume_handle);
        let source_disk_size = source_disk
            .map(|disk| disk.disk_size)
            .or_else(|| volume_info.map(|info| info.total_size))
            .unwrap_or(0);
        let fallback_block_size = volume_info.map_or(512, |info| info.block_size);
        self.scan_directory_entries(
            volume_handle,
            volume_index,
            source_disk,
            source_disk_size,
            fallback_block_size,
            &mut dir,
            &normalized,
            extensions,
            config,
            config.max_search_level,
            0,
            &mut files,
        )?;
        Ok(files)
    }

    fn scan_block_filesystem_volumes(
        &self,
        volume_index_base: usize,
        simple_fs_handles: &[Handle],
        default_search_paths: &[&str],
        extensions: &[&str],
    ) -> uefi::Result<Vec<IsoFile>> {
        let block_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))?;
        let mut files = Vec::new();
        let mut block_volume_index = 0usize;
        let all_block_handles: Vec<Handle> = block_handles.iter().copied().collect();

        for handle in all_block_handles.iter().copied() {
            if handle_list_contains(simple_fs_handles, handle) {
                continue;
            }

            let block_io = match self.bt.open_protocol_exclusive::<BlockIO>(handle) {
                Ok(block_io) => block_io,
                Err(_) => continue,
            };
            let media = block_io.media();
            if !media.is_media_present() {
                continue;
            }
            let block_size = media.block_size();
            if block_size == 0 {
                continue;
            }

            let Some(uefi_io) = UefiBlockIo::new(&block_io) else {
                continue;
            };
            let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
            let mut boot_sector = match alloc_buffer_for_block(block_size) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
            if shared.read_blocks(0, &mut boot_sector).is_err() {
                continue;
            }

            let fs_type = detect_fs_type(&boot_sector);
            if !matches!(
                fs_type,
                FileSystemType::Fat32 | FileSystemType::ExFat | FileSystemType::Ntfs
            ) {
                let scanned = self.scan_partitioned_block_device(
                    handle,
                    &all_block_handles,
                    volume_index_base,
                    &mut block_volume_index,
                    &block_io,
                    shared.clone(),
                    &boot_sector,
                    default_search_paths,
                    extensions,
                    &mut files,
                );
                if scanned > 0 {
                    continue;
                }

                let volume_index = volume_index_base + block_volume_index;
                let source_disk = self.resolve_source_disk_identity(handle);
                let source_disk_size = source_disk
                    .map(|disk| disk.disk_size)
                    .or_else(|| block_io_info(&block_io).map(|info| info.total_size))
                    .unwrap_or(0);
                if self.scan_unknown_block_filesystem_volume(
                    handle,
                    volume_index,
                    source_disk,
                    source_disk_size,
                    &block_io,
                    shared,
                    default_search_paths,
                    extensions,
                    0,
                    &mut files,
                ) {
                    block_volume_index += 1;
                }
                continue;
            }

            let volume_index = volume_index_base + block_volume_index;
            let source_disk = self.resolve_source_disk_identity(handle);
            let source_disk_size = source_disk
                .map(|disk| disk.disk_size)
                .or_else(|| block_io_info(&block_io).map(|info| info.total_size))
                .unwrap_or(0);

            match fs_type {
                FileSystemType::Fat32 => {
                    let fs = match Fat32::open(shared.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring FAT32 BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                FileSystemType::ExFat => {
                    let fs = match ExFat::open(shared.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring exFAT BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                FileSystemType::Ntfs => {
                    let fs = match Ntfs::open(shared) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring NTFS BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                _ => {}
            }
            block_volume_index += 1;
        }

        Ok(files)
    }

    fn scan_unknown_block_filesystem_volume(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        source_disk_size: u64,
        block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        default_search_paths: &[&str],
        extensions: &[&str],
        extent_lba_offset: u64,
        files: &mut Vec<IsoFile>,
    ) -> bool {
        if let Ok(fs) = Udf::open(shared.clone()) {
            self.scan_block_filesystem_paths(
                volume_handle,
                volume_index,
                source_disk,
                source_disk_size,
                block_io,
                &fs,
                default_search_paths,
                extensions,
                extent_lba_offset,
                files,
            );
            return true;
        }

        if let Ok(fs) = Iso9660::open(shared) {
            self.scan_block_filesystem_paths(
                volume_handle,
                volume_index,
                source_disk,
                source_disk_size,
                block_io,
                &fs,
                default_search_paths,
                extensions,
                extent_lba_offset,
                files,
            );
            return true;
        }

        false
    }

    fn scan_partitioned_block_device(
        &self,
        physical_handle: Handle,
        all_block_handles: &[Handle],
        volume_index_base: usize,
        block_volume_index: &mut usize,
        block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        first_block: &[u8],
        default_search_paths: &[&str],
        extensions: &[&str],
        files: &mut Vec<IsoFile>,
    ) -> usize {
        let Some(volume_info) = block_io_info(block_io) else {
            return 0;
        };
        let partitions = discover_partition_candidates(shared.clone(), first_block);
        if partitions.is_empty() {
            return 0;
        }

        let exposed = self.exposed_child_partitions(physical_handle, all_block_handles);
        let mut scanned = 0usize;
        for partition in partitions {
            if exposed
                .iter()
                .any(|range| range.matches(partition.start_lba, partition.block_count))
            {
                continue;
            }
            if partition.block_count == 0
                || partition
                    .start_lba
                    .checked_add(partition.block_count)
                    .map_or(true, |end| end > shared.total_blocks())
            {
                continue;
            }

            let partition_io: nextboot_fs::SharedBlockIo = Rc::new(PartitionBlockIo::new(
                shared.clone(),
                partition.start_lba,
                partition.block_count,
            ));
            let mut boot_sector = match alloc_buffer_for_block(partition_io.block_size()) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
            if partition_io.read_blocks(0, &mut boot_sector).is_err() {
                continue;
            }
            let fs_type = detect_fs_type(&boot_sector);

            let volume_index = volume_index_base + *block_volume_index;
            let source_disk = partition_source_disk_identity(first_block, volume_info, partition);
            let source_disk_size = source_disk
                .map(|disk| disk.disk_size)
                .unwrap_or(volume_info.total_size);

            match fs_type {
                FileSystemType::Fat32 => {
                    let fs = match Fat32::open(partition_io.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring FAT32 partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                FileSystemType::ExFat => {
                    let fs = match ExFat::open(partition_io.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring exFAT partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                FileSystemType::Ntfs => {
                    let fs = match Ntfs::open(partition_io) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring NTFS partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                _ => {
                    if !self.scan_unknown_block_filesystem_volume(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        partition_io,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    ) {
                        continue;
                    }
                }
            }

            *block_volume_index += 1;
            scanned += 1;
        }

        scanned
    }

    fn exposed_child_partitions(
        &self,
        physical_handle: Handle,
        all_block_handles: &[Handle],
    ) -> Vec<PartitionRange> {
        let Some(physical_path) = self.handle_device_path_bytes(physical_handle) else {
            return Vec::new();
        };
        let mut ranges = Vec::new();

        for handle in all_block_handles.iter().copied() {
            if handle.as_ptr() == physical_handle.as_ptr() {
                continue;
            }
            let Some(path) = self.handle_device_path_bytes(handle) else {
                continue;
            };
            let Some(hard_drive) = parse_last_hard_drive_device_path(&path) else {
                continue;
            };
            let Some(parent_path) = parent_device_path_bytes(&path, &hard_drive) else {
                continue;
            };
            if parent_path != physical_path {
                continue;
            }
            if ranges.try_reserve_exact(1).is_err() {
                break;
            }
            ranges.push(PartitionRange {
                start_lba: hard_drive.partition_start_lba,
                block_count: hard_drive.partition_size_blocks,
            });
        }

        ranges
    }

    fn scan_block_filesystem_paths<F: FileSystem>(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        source_disk_size: u64,
        block_io: &BlockIO,
        fs: &F,
        default_search_paths: &[&str],
        extensions: &[&str],
        extent_lba_offset: u64,
        files: &mut Vec<IsoFile>,
    ) {
        let config = self.load_block_ventoy_config(fs);
        let search_paths = config.search_roots(default_search_paths);

        for search_path in &search_paths {
            let _ = self.scan_block_filesystem_path(
                volume_handle,
                volume_index,
                source_disk,
                source_disk_size,
                block_io,
                fs,
                search_path,
                extensions,
                &config,
                extent_lba_offset,
                config.max_search_level,
                0,
                files,
            );
        }
    }

    fn scan_block_filesystem_path<F: FileSystem>(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        source_disk_size: u64,
        block_io: &BlockIO,
        fs: &F,
        display_path: &str,
        extensions: &[&str],
        config: &VentoyConfig,
        extent_lba_offset: u64,
        max_search_level: Option<usize>,
        depth: usize,
        files: &mut Vec<IsoFile>,
    ) -> Result<(), FsError> {
        let normalized = normalize_scan_path(display_path);
        if is_ventoy_plugin_tree_path(&normalized) {
            return Ok(());
        }
        let entries = fs.read_dir(&normalized)?;

        for entry in entries {
            if entry.name.is_empty() || entry.name == "." || entry.name == ".." {
                continue;
            }

            let full_path = join_display_path(&normalized, &entry.name);
            if entry.is_dir {
                if !should_descend_into_directory(depth, max_search_level)
                    || is_hidden_tree(&entry.name)
                    || is_ventoy_plugin_tree_path(&full_path)
                {
                    continue;
                }
                let _ = self.scan_block_filesystem_path(
                    volume_handle,
                    volume_index,
                    source_disk,
                    source_disk_size,
                    block_io,
                    fs,
                    &full_path,
                    extensions,
                    config,
                    extent_lba_offset,
                    max_search_level,
                    depth + 1,
                    files,
                );
                continue;
            }

            if entry.is_hidden() || entry.is_system() {
                continue;
            }

            if config.filter_dot_underscore && is_dot_underscore_file(&entry.name) {
                continue;
            }

            if is_default_uefi_bootloader_path(&full_path) {
                continue;
            }

            if vlnk::is_vlnk_name(&entry.name) {
                if config.supports_image_name(&entry.name) && config.allows_image_path(&full_path) {
                    if let Some(file) = self.resolve_block_vlnk_file(
                        volume_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        fs,
                        &full_path,
                        entry.size,
                        config,
                        extent_lba_offset,
                    ) {
                        files.push(file);
                    }
                }
                continue;
            }

            if has_supported_extension(&entry.name, extensions)
                && config.supports_image_name(&entry.name)
                && config.allows_image_path(&full_path)
            {
                let image_format = ImageFormat::detect_from_path(&full_path);
                let metadata = self
                    .resolve_block_image_metadata(
                        block_io,
                        fs,
                        &full_path,
                        entry.size,
                        image_format,
                        extent_lba_offset,
                    )
                    .unwrap_or_else(|| ResolvedImageMetadata {
                        block_size: fs.block_size(),
                        extents: Vec::new(),
                        boot_info: None,
                        is_udf: false,
                        wim_info: None,
                        image_format,
                        virtual_size: entry.size,
                        virtual_block_size: default_virtual_block_size(image_format),
                    });
                let start_lba = metadata
                    .extents
                    .first()
                    .map_or(0, |extent| extent.physical_lba);
                let os_type =
                    self.detect_image_os_type(&full_path, metadata.image_format, metadata.wim_info);

                files.push(IsoFile {
                    path: full_path.clone(),
                    menu_alias: config.menu_alias_for(&full_path).map(ToString::to_string),
                    ventoy_menu_class: config
                        .menu_class_for_image(&full_path)
                        .map(ToString::to_string),
                    ventoy_menu_tip: config.menu_tip_for_image(&full_path).cloned(),
                    ventoy_default_image: config.default_image_matches(&full_path),
                    ventoy_menu_timeout: config.menu_timeout,
                    ventoy_linux_remount: config.linux_remount,
                    ventoy_windows_cd_prompt: config.windows_cd_prompt,
                    ventoy_windows_uefi_resolution_lock: config.windows_uefi_resolution_lock,
                    ventoy_windows11_bypass_check: config.windows11_bypass_check,
                    ventoy_windows11_bypass_nro: config.windows11_bypass_nro,
                    ventoy_password: config.image_password_for(&full_path).cloned(),
                    ventoy_boot_password: config.password.boot.clone(),
                    ventoy_plugin: config.image_plugin_for(&full_path),
                    size: entry.size,
                    virtual_size: metadata.virtual_size,
                    virtual_block_size: metadata.virtual_block_size,
                    volume_handle,
                    asset_volume_handle: volume_handle,
                    volume_index,
                    block_size: metadata.block_size,
                    start_lba,
                    extents: metadata.extents,
                    os_type,
                    image_format: metadata.image_format,
                    boot_info: metadata.boot_info,
                    is_udf: metadata.is_udf,
                    wim_info: metadata.wim_info,
                    source_disk,
                    asset_source_disk: source_disk,
                    source_disk_size,
                    is_vlnk: false,
                    vlnk_target_path: None,
                });
            }
        }

        Ok(())
    }

    fn scan_directory_entries(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        source_disk_size: u64,
        fallback_block_size: u32,
        dir: &mut Directory,
        display_path: &str,
        extensions: &[&str],
        config: &VentoyConfig,
        max_search_level: Option<usize>,
        depth: usize,
        files: &mut Vec<IsoFile>,
    ) -> uefi::Result<()> {
        if is_ventoy_plugin_tree_path(display_path) {
            return Ok(());
        }

        while let Some(entry) = dir.read_entry_boxed()? {
            let name = cstr16_to_string(entry.file_name());

            if name.is_empty() || name == "." || name == ".." {
                continue;
            }

            let full_path = join_display_path(display_path, &name);

            if entry.is_directory() {
                if !should_descend_into_directory(depth, max_search_level)
                    || is_hidden_tree(&name)
                    || is_ventoy_plugin_tree_path(&full_path)
                {
                    continue;
                }

                if let Ok(mut child) = open_directory(dir, &name) {
                    let _ = self.scan_directory_entries(
                        volume_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        fallback_block_size,
                        &mut child,
                        &full_path,
                        extensions,
                        config,
                        max_search_level,
                        depth + 1,
                        files,
                    );
                }
                continue;
            }

            if config.filter_dot_underscore && is_dot_underscore_file(&name) {
                continue;
            }

            if is_default_uefi_bootloader_path(&full_path) {
                continue;
            }

            if vlnk::is_vlnk_name(&name) {
                if config.supports_image_name(&name) && config.allows_image_path(&full_path) {
                    if let Some(file) = self.resolve_uefi_vlnk_file(
                        volume_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        fallback_block_size,
                        dir,
                        &name,
                        &full_path,
                        entry.file_size(),
                        config,
                    ) {
                        files.push(file);
                    }
                }
                continue;
            }

            if has_supported_extension(&name, extensions)
                && config.supports_image_name(&name)
                && config.allows_image_path(&full_path)
            {
                let image_format = ImageFormat::detect_from_path(&full_path);
                let ResolvedImageMetadata {
                    block_size,
                    extents,
                    boot_info,
                    is_udf,
                    wim_info,
                    image_format,
                    virtual_size,
                    virtual_block_size,
                } = self
                    .resolve_image_metadata(
                        volume_handle,
                        &full_path,
                        entry.file_size(),
                        image_format,
                    )
                    .unwrap_or_else(|| ResolvedImageMetadata {
                        block_size: fallback_block_size,
                        extents: Vec::new(),
                        boot_info: None,
                        is_udf: false,
                        wim_info: None,
                        image_format,
                        virtual_size: entry.file_size(),
                        virtual_block_size: default_virtual_block_size(image_format),
                    });
                let start_lba = extents.first().map_or(0, |extent| extent.physical_lba);
                let os_type = self.detect_image_os_type(&full_path, image_format, wim_info);

                files.push(IsoFile {
                    path: full_path.clone(),
                    menu_alias: config.menu_alias_for(&full_path).map(ToString::to_string),
                    ventoy_menu_class: config
                        .menu_class_for_image(&full_path)
                        .map(ToString::to_string),
                    ventoy_menu_tip: config.menu_tip_for_image(&full_path).cloned(),
                    ventoy_default_image: config.default_image_matches(&full_path),
                    ventoy_menu_timeout: config.menu_timeout,
                    ventoy_linux_remount: config.linux_remount,
                    ventoy_windows_cd_prompt: config.windows_cd_prompt,
                    ventoy_windows_uefi_resolution_lock: config.windows_uefi_resolution_lock,
                    ventoy_windows11_bypass_check: config.windows11_bypass_check,
                    ventoy_windows11_bypass_nro: config.windows11_bypass_nro,
                    ventoy_password: config.image_password_for(&full_path).cloned(),
                    ventoy_boot_password: config.password.boot.clone(),
                    ventoy_plugin: config.image_plugin_for(&full_path),
                    size: entry.file_size(),
                    virtual_size,
                    virtual_block_size,
                    volume_handle,
                    asset_volume_handle: volume_handle,
                    volume_index,
                    block_size,
                    start_lba,
                    extents,
                    os_type,
                    image_format,
                    boot_info,
                    is_udf,
                    wim_info,
                    source_disk,
                    asset_source_disk: source_disk,
                    source_disk_size,
                    is_vlnk: false,
                    vlnk_target_path: None,
                });
            }
        }

        Ok(())
    }

    fn resolve_uefi_vlnk_file(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        asset_source_disk_size: u64,
        _fallback_block_size: u32,
        dir: &mut Directory,
        name: &str,
        link_path: &str,
        link_size: u64,
        config: &VentoyConfig,
    ) -> Option<IsoFile> {
        let data = match read_uefi_regular_file(dir, name, link_size) {
            Ok(data) => data,
            Err(status) => {
                log::warn!("Ventoy VLNK {} was not loaded: {:?}", link_path, status);
                return None;
            }
        };
        let vlnk = match vlnk::parse_vlnk(&data) {
            Ok(vlnk) => vlnk,
            Err(err) => {
                log::warn!("Ventoy VLNK {} is invalid: {:?}", link_path, err);
                return None;
            }
        };

        self.resolve_vlnk_target(
            asset_volume_handle,
            asset_volume_index,
            asset_source_disk,
            asset_source_disk_size,
            link_path,
            config,
            &vlnk,
        )
    }

    fn resolve_block_vlnk_file<F: FileSystem>(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        asset_source_disk_size: u64,
        current_block_io: &BlockIO,
        current_fs: &F,
        link_path: &str,
        link_size: u64,
        config: &VentoyConfig,
        current_extent_lba_offset: u64,
    ) -> Option<IsoFile> {
        if link_size != vlnk::VLNK_FILE_LEN as u64 {
            log::warn!("Ventoy VLNK {} has invalid size {}", link_path, link_size);
            return None;
        }
        let mut data = Vec::new();
        let file_size = usize::try_from(link_size).ok()?;
        data.try_reserve_exact(file_size).ok()?;
        data.resize(file_size, 0);
        let read = current_fs.read_file(link_path, 0, &mut data).ok()?;
        data.truncate(read);

        let vlnk = match vlnk::parse_vlnk(&data) {
            Ok(vlnk) => vlnk,
            Err(err) => {
                log::warn!("Ventoy VLNK {} is invalid: {:?}", link_path, err);
                return None;
            }
        };

        if vlnk_matches_source_disk(asset_source_disk, &vlnk) {
            let target_path = normalize_vlnk_target_path(&vlnk.filepath);
            if let Some(file) = self.build_vlnk_iso_file_from_fs(
                asset_volume_handle,
                asset_volume_index,
                asset_source_disk,
                asset_source_disk_size,
                asset_volume_handle,
                asset_source_disk,
                asset_source_disk_size,
                current_block_io,
                current_fs,
                &target_path,
                link_path,
                config,
                current_extent_lba_offset,
            ) {
                return Some(file);
            }
        }

        self.resolve_vlnk_target(
            asset_volume_handle,
            asset_volume_index,
            asset_source_disk,
            asset_source_disk_size,
            link_path,
            config,
            &vlnk,
        )
    }

    fn resolve_vlnk_target(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        asset_source_disk_size: u64,
        link_path: &str,
        config: &VentoyConfig,
        vlnk: &VentoyVlnk,
    ) -> Option<IsoFile> {
        let target_path = normalize_vlnk_target_path(&vlnk.filepath);
        let block_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))
            .ok()?;
        let all_block_handles: Vec<Handle> = block_handles.iter().copied().collect();

        for handle in all_block_handles {
            let block_io = match self.bt.open_protocol_exclusive::<BlockIO>(handle) {
                Ok(block_io) => block_io,
                Err(_) => continue,
            };
            let media = block_io.media();
            if !media.is_media_present() || media.block_size() == 0 {
                continue;
            }
            let Some(uefi_io) = UefiBlockIo::new(&block_io) else {
                continue;
            };
            let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
            let mut first_block = match alloc_buffer_for_block(media.block_size()) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
            if shared.read_blocks(0, &mut first_block).is_err() {
                continue;
            }

            let direct_source_disk = self.resolve_source_disk_identity(handle);
            let direct_source_disk_size = direct_source_disk
                .map(|disk| disk.disk_size)
                .or_else(|| block_io_info(&block_io).map(|info| info.total_size))
                .unwrap_or(0);
            if vlnk_matches_source_disk(direct_source_disk, vlnk) {
                if let Some(file) = self.resolve_vlnk_on_detected_fs(
                    asset_volume_handle,
                    asset_volume_index,
                    asset_source_disk,
                    asset_source_disk_size,
                    handle,
                    direct_source_disk,
                    direct_source_disk_size,
                    &block_io,
                    shared.clone(),
                    detect_fs_type(&first_block),
                    &target_path,
                    link_path,
                    config,
                    0,
                ) {
                    return Some(file);
                }
            }

            let Some(volume_info) = block_io_info(&block_io) else {
                continue;
            };
            let disk_signature = match first_block.get(0x1b8..0x1bc) {
                Some(signature) => signature,
                None => continue,
            };
            if disk_signature != vlnk.disk_signature {
                continue;
            }

            let partitions = discover_partition_candidates(shared.clone(), &first_block);
            for partition in partitions {
                if !vlnk_matches_partition(partition, media.block_size(), vlnk) {
                    continue;
                }
                if partition.block_count == 0
                    || partition
                        .start_lba
                        .checked_add(partition.block_count)
                        .map_or(true, |end| end > shared.total_blocks())
                {
                    continue;
                }

                let target_source_disk =
                    partition_source_disk_identity(&first_block, volume_info, partition);
                let target_source_disk_size = target_source_disk
                    .map(|disk| disk.disk_size)
                    .unwrap_or(volume_info.total_size);
                let partition_io: nextboot_fs::SharedBlockIo = Rc::new(PartitionBlockIo::new(
                    shared.clone(),
                    partition.start_lba,
                    partition.block_count,
                ));
                let mut boot_sector = match alloc_buffer_for_block(partition_io.block_size()) {
                    Ok(buf) => buf,
                    Err(_) => continue,
                };
                if partition_io.read_blocks(0, &mut boot_sector).is_err() {
                    continue;
                }

                if let Some(file) = self.resolve_vlnk_on_detected_fs(
                    asset_volume_handle,
                    asset_volume_index,
                    asset_source_disk,
                    asset_source_disk_size,
                    handle,
                    target_source_disk,
                    target_source_disk_size,
                    &block_io,
                    partition_io,
                    detect_fs_type(&boot_sector),
                    &target_path,
                    link_path,
                    config,
                    partition.start_lba,
                ) {
                    return Some(file);
                }
            }
        }

        log::warn!(
            "Ventoy VLNK {} target was not found: sig={:02x}{:02x}{:02x}{:02x} offset={} path={}",
            link_path,
            vlnk.disk_signature[3],
            vlnk.disk_signature[2],
            vlnk.disk_signature[1],
            vlnk.disk_signature[0],
            vlnk.part_offset_bytes,
            vlnk.filepath
        );
        None
    }

    fn resolve_vlnk_on_detected_fs(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        asset_source_disk_size: u64,
        target_volume_handle: Handle,
        target_source_disk: Option<SourceDiskIdentity>,
        target_source_disk_size: u64,
        target_block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        fs_type: FileSystemType,
        target_path: &str,
        link_path: &str,
        config: &VentoyConfig,
        extent_lba_offset: u64,
    ) -> Option<IsoFile> {
        match fs_type {
            FileSystemType::Fat32 => {
                let fs = Fat32::open(shared).ok()?;
                self.build_vlnk_iso_file_from_fs(
                    asset_volume_handle,
                    asset_volume_index,
                    asset_source_disk,
                    asset_source_disk_size,
                    target_volume_handle,
                    target_source_disk,
                    target_source_disk_size,
                    target_block_io,
                    &fs,
                    target_path,
                    link_path,
                    config,
                    extent_lba_offset,
                )
            }
            FileSystemType::ExFat => {
                let fs = ExFat::open(shared).ok()?;
                self.build_vlnk_iso_file_from_fs(
                    asset_volume_handle,
                    asset_volume_index,
                    asset_source_disk,
                    asset_source_disk_size,
                    target_volume_handle,
                    target_source_disk,
                    target_source_disk_size,
                    target_block_io,
                    &fs,
                    target_path,
                    link_path,
                    config,
                    extent_lba_offset,
                )
            }
            FileSystemType::Ntfs => {
                let fs = Ntfs::open(shared).ok()?;
                self.build_vlnk_iso_file_from_fs(
                    asset_volume_handle,
                    asset_volume_index,
                    asset_source_disk,
                    asset_source_disk_size,
                    target_volume_handle,
                    target_source_disk,
                    target_source_disk_size,
                    target_block_io,
                    &fs,
                    target_path,
                    link_path,
                    config,
                    extent_lba_offset,
                )
            }
            _ => Udf::open(shared.clone())
                .ok()
                .and_then(|fs| {
                    self.build_vlnk_iso_file_from_fs(
                        asset_volume_handle,
                        asset_volume_index,
                        asset_source_disk,
                        asset_source_disk_size,
                        target_volume_handle,
                        target_source_disk,
                        target_source_disk_size,
                        target_block_io,
                        &fs,
                        target_path,
                        link_path,
                        config,
                        extent_lba_offset,
                    )
                })
                .or_else(|| {
                    Iso9660::open(shared).ok().and_then(|fs| {
                        self.build_vlnk_iso_file_from_fs(
                            asset_volume_handle,
                            asset_volume_index,
                            asset_source_disk,
                            asset_source_disk_size,
                            target_volume_handle,
                            target_source_disk,
                            target_source_disk_size,
                            target_block_io,
                            &fs,
                            target_path,
                            link_path,
                            config,
                            extent_lba_offset,
                        )
                    })
                }),
        }
    }

    fn build_vlnk_iso_file_from_fs<F: FileSystem>(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        _asset_source_disk_size: u64,
        target_volume_handle: Handle,
        target_source_disk: Option<SourceDiskIdentity>,
        target_source_disk_size: u64,
        target_block_io: &BlockIO,
        fs: &F,
        target_path: &str,
        link_path: &str,
        config: &VentoyConfig,
        extent_lba_offset: u64,
    ) -> Option<IsoFile> {
        let info = fs.stat(target_path).ok()?;
        if info.is_dir {
            return None;
        }
        let mut image_format = ImageFormat::detect_from_path(target_path);
        if image_format == ImageFormat::Unknown {
            image_format = ImageFormat::detect_from_path(vlnk::target_image_format_path(link_path));
        }
        let metadata = self.resolve_block_image_metadata(
            target_block_io,
            fs,
            target_path,
            info.size,
            image_format,
            extent_lba_offset,
        )?;
        let start_lba = metadata
            .extents
            .first()
            .map_or(0, |extent| extent.physical_lba);
        let os_type =
            self.detect_image_os_type(target_path, metadata.image_format, metadata.wim_info);

        log::info!(
            "Resolved Ventoy VLNK {} -> {} ({} bytes, {})",
            link_path,
            target_path,
            info.size,
            metadata.image_format
        );

        Some(IsoFile {
            path: link_path.to_string(),
            menu_alias: config.menu_alias_for(link_path).map(ToString::to_string),
            ventoy_menu_class: config
                .menu_class_for_image(link_path)
                .map(ToString::to_string),
            ventoy_menu_tip: config.menu_tip_for_image(link_path).cloned(),
            ventoy_default_image: config.default_image_matches(link_path),
            ventoy_menu_timeout: config.menu_timeout,
            ventoy_linux_remount: config.linux_remount,
            ventoy_windows_cd_prompt: config.windows_cd_prompt,
            ventoy_windows_uefi_resolution_lock: config.windows_uefi_resolution_lock,
            ventoy_windows11_bypass_check: config.windows11_bypass_check,
            ventoy_windows11_bypass_nro: config.windows11_bypass_nro,
            ventoy_password: config.image_password_for(link_path).cloned(),
            ventoy_boot_password: config.password.boot.clone(),
            ventoy_plugin: config.image_plugin_for(link_path),
            size: info.size,
            virtual_size: metadata.virtual_size,
            virtual_block_size: metadata.virtual_block_size,
            volume_handle: target_volume_handle,
            asset_volume_handle,
            volume_index: asset_volume_index,
            block_size: metadata.block_size,
            start_lba,
            extents: metadata.extents,
            os_type,
            image_format: metadata.image_format,
            boot_info: metadata.boot_info,
            is_udf: metadata.is_udf,
            wim_info: metadata.wim_info,
            source_disk: target_source_disk,
            asset_source_disk,
            source_disk_size: target_source_disk_size,
            is_vlnk: true,
            vlnk_target_path: Some(target_path.to_string()),
        })
    }

    fn load_ventoy_config(&self, fs: &mut SimpleFileSystem) -> VentoyConfig {
        match self.read_ventoy_config(fs) {
            Ok(config) => config,
            Err(VentoyConfigError::NotFound) => VentoyConfig::default(),
            Err(err) => {
                log::warn!("Ignoring {}: {:?}", VENTOY_CONFIG_PATH, err);
                VentoyConfig::default()
            }
        }
    }

    fn read_ventoy_config(
        &self,
        fs: &mut SimpleFileSystem,
    ) -> Result<VentoyConfig, VentoyConfigError> {
        let mut root = fs
            .open_volume()
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        let uefi_path = to_uefi_relative_path(VENTOY_CONFIG_PATH);
        let c_path =
            CString16::try_from(uefi_path.as_str()).map_err(|_| VentoyConfigError::InvalidJson)?;
        let handle = root
            .open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())
            .map_err(|_| VentoyConfigError::NotFound)?;
        let mut file = handle
            .into_regular_file()
            .ok_or(VentoyConfigError::InvalidJson)?;
        let info = file
            .get_boxed_info::<FileInfo>()
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        let file_size =
            usize::try_from(info.file_size()).map_err(|_| VentoyConfigError::FileTooLarge)?;
        if file_size > VENTOY_CONFIG_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        data.resize(file_size, 0);
        let mut offset = 0;
        while offset < data.len() {
            let read = file
                .read(&mut data[offset..])
                .map_err(|_| VentoyConfigError::InvalidJson)?;
            if read == 0 {
                break;
            }
            offset += read;
        }
        data.truncate(offset);

        VentoyConfig::parse(&data)
    }

    fn load_block_ventoy_config<F: FileSystem>(&self, fs: &F) -> VentoyConfig {
        match self.read_block_ventoy_config(fs) {
            Ok(config) => config,
            Err(VentoyConfigError::NotFound) => VentoyConfig::default(),
            Err(err) => {
                log::warn!("Ignoring {} {}: {:?}", F::FS_TYPE, VENTOY_CONFIG_PATH, err);
                VentoyConfig::default()
            }
        }
    }

    fn read_block_ventoy_config<F: FileSystem>(
        &self,
        fs: &F,
    ) -> Result<VentoyConfig, VentoyConfigError> {
        let info = fs.stat(VENTOY_CONFIG_PATH).map_err(|err| match err {
            FsError::FileNotFound | FsError::DirectoryNotFound => VentoyConfigError::NotFound,
            _ => VentoyConfigError::InvalidJson,
        })?;
        if info.is_dir {
            return Err(VentoyConfigError::InvalidJson);
        }

        let file_size = usize::try_from(info.size).map_err(|_| VentoyConfigError::FileTooLarge)?;
        if file_size > VENTOY_CONFIG_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        data.resize(file_size, 0);
        let read = fs
            .read_file(VENTOY_CONFIG_PATH, 0, &mut data)
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        data.truncate(read);

        VentoyConfig::parse(&data)
    }

    fn volume_block_info(&self, volume_handle: Handle) -> Option<VolumeBlockInfo> {
        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(volume_handle)
            .ok()?;
        block_io_info(&block_io)
    }

    fn resolve_image_metadata(
        &self,
        volume_handle: Handle,
        path: &str,
        size: u64,
        image_format: ImageFormat,
    ) -> Option<ResolvedImageMetadata> {
        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(volume_handle)
            .ok()?;
        let media = block_io.media();
        if !media.is_media_present() {
            return None;
        }

        let block_size = media.block_size();
        let uefi_io = UefiBlockIo::new(&block_io)?;
        let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
        let mut boot_sector = alloc::vec![0u8; block_size as usize];
        shared.read_blocks(0, &mut boot_sector).ok()?;

        let (block_size, extents) =
            source_file_extents_from_detected_fs(shared, detect_fs_type(&boot_sector), path)?;

        let extents: Vec<IsoExtent> = extents.into_iter().map(IsoExtent::from).collect();
        let (image_format, virtual_size, virtual_block_size) =
            self.detect_image_virtual_metadata(&block_io, block_size, size, &extents, image_format);
        let (boot_info, is_udf) = if image_format.is_iso() {
            self.resolve_iso_metadata(&block_io, block_size, size, &extents)
        } else {
            (None, false)
        };
        let wim_info = if image_format.is_wim_container() {
            self.read_wim_boot_info(&block_io, block_size, size, &extents)
        } else {
            None
        };

        Some(ResolvedImageMetadata {
            block_size,
            extents,
            boot_info,
            is_udf,
            wim_info,
            image_format,
            virtual_size,
            virtual_block_size,
        })
    }

    fn resolve_block_image_metadata<F: FileSystem>(
        &self,
        block_io: &BlockIO,
        fs: &F,
        path: &str,
        size: u64,
        image_format: ImageFormat,
        extent_lba_offset: u64,
    ) -> Option<ResolvedImageMetadata> {
        let block_size = fs.block_size();
        let extents: Vec<IsoExtent> = fs
            .file_extents(path)
            .ok()?
            .into_iter()
            .map(IsoExtent::from)
            .collect();
        let read_extents = offset_extents_for_physical_read(&extents, extent_lba_offset)?;
        let (image_format, virtual_size, virtual_block_size) = self.detect_image_virtual_metadata(
            block_io,
            block_size,
            size,
            &read_extents,
            image_format,
        );
        let (boot_info, is_udf) = if image_format.is_iso() {
            self.resolve_iso_metadata(block_io, block_size, size, &read_extents)
        } else {
            (None, false)
        };
        let wim_info = if image_format.is_wim_container() {
            self.read_wim_boot_info(block_io, block_size, size, &read_extents)
        } else {
            None
        };

        Some(ResolvedImageMetadata {
            block_size,
            extents,
            boot_info,
            is_udf,
            wim_info,
            image_format,
            virtual_size,
            virtual_block_size,
        })
    }

    fn resolve_source_disk_identity(&self, volume_handle: Handle) -> Option<SourceDiskIdentity> {
        let volume_device_path = self.handle_device_path_bytes(volume_handle);
        let hard_drive = volume_device_path
            .as_deref()
            .and_then(parse_last_hard_drive_device_path);
        let parent_handle = match (volume_device_path.as_deref(), hard_drive.as_ref()) {
            (Some(path), Some(info)) => self.locate_parent_block_io(path, info)?,
            (_, None) => volume_handle,
            _ => return None,
        };

        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(parent_handle)
            .ok()?;
        let media = block_io.media();
        if hard_drive.is_none() && media.is_logical_partition() {
            return None;
        }

        let block_size = media.block_size();
        if block_size < 512 {
            return None;
        }
        let total_blocks = media.last_block().checked_add(1)?;
        let disk_size = total_blocks.checked_mul(u64::from(block_size))?;
        let block_len = usize::try_from(block_size).ok()?;
        let mut first_block = Vec::new();
        first_block.try_reserve_exact(block_len).ok()?;
        first_block.resize(block_len, 0);
        block_io
            .read_blocks(media.media_id(), 0, &mut first_block)
            .ok()?;

        build_source_disk_identity(&first_block, disk_size, block_size, hard_drive)
    }

    fn locate_parent_block_io(
        &self,
        volume_device_path: &[u8],
        hard_drive: &crate::source_disk::HardDriveDevicePathInfo,
    ) -> Option<Handle> {
        let parent_path = parent_device_path_bytes(volume_device_path, hard_drive)?;
        let mut device_path =
            unsafe { DevicePath::from_ffi_ptr(parent_path.as_ptr().cast::<FfiDevicePath>()) };
        self.bt.locate_device_path::<BlockIO>(&mut device_path).ok()
    }

    fn handle_device_path_bytes(&self, handle: Handle) -> Option<Vec<u8>> {
        let device_path = self.bt.open_protocol_exclusive::<DevicePath>(handle).ok()?;
        device_path_to_vec(&device_path)
    }

    fn detect_image_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        image_format: ImageFormat,
    ) -> (ImageFormat, u64, Option<u32>) {
        match image_format {
            ImageFormat::Vhd => {
                match self.read_image_tail(block_io, source_block_size, file_size, extents, 512) {
                    Some(footer) => parse_vhd_footer(&footer)
                        .map(|info| {
                            let virtual_size = if info.image_format == ImageFormat::FixedVhd {
                                info.virtual_size.min(file_size.saturating_sub(512))
                            } else {
                                info.virtual_size
                            };
                            (info.image_format, virtual_size, Some(512))
                        })
                        .unwrap_or((
                            image_format,
                            file_size,
                            default_virtual_block_size(image_format),
                        )),
                    None => (
                        image_format,
                        file_size,
                        default_virtual_block_size(image_format),
                    ),
                }
            }
            ImageFormat::Vhdx => self
                .read_vhdx_virtual_metadata(block_io, source_block_size, file_size, extents)
                .map(|metadata| {
                    (
                        image_format,
                        metadata.virtual_disk_size,
                        Some(metadata.logical_sector_size),
                    )
                })
                .unwrap_or((
                    image_format,
                    file_size,
                    default_virtual_block_size(image_format),
                )),
            ImageFormat::Vdi => self
                .read_vdi_virtual_metadata(block_io, source_block_size, file_size, extents)
                .map(|metadata| {
                    (
                        image_format,
                        metadata.virtual_disk_size,
                        Some(metadata.sector_size),
                    )
                })
                .unwrap_or((
                    image_format,
                    file_size,
                    default_virtual_block_size(image_format),
                )),
            _ => (
                image_format,
                file_size,
                default_virtual_block_size(image_format),
            ),
        }
    }

    fn read_vhdx_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<vhdx::VhdxMetadata> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            vhdx::VHDX_HEADER_SECTION_SIZE,
        )?;
        let regions = vhdx::parse_vhdx_regions(&header)?;
        if regions.metadata_length > usize::MAX as u64 {
            return None;
        }
        let metadata = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            regions.metadata_offset,
            regions.metadata_length as usize,
        )?;
        vhdx::parse_vhdx_metadata(&metadata)
    }

    fn read_vdi_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<vdi::VdiMetadata> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            vdi::VDI_HEADER_SIZE,
        )?;
        vdi::parse_vdi_metadata(&header)
    }

    fn read_wim_boot_info(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<WimBootInfo> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            wim::WIM_HEADER_SIZE,
        )?;
        wim::parse_wim_metadata(&header).map(WimBootInfo::from)
    }

    fn read_image_tail(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        tail_len: usize,
    ) -> Option<Vec<u8>> {
        if extents.is_empty() || tail_len == 0 || file_size < tail_len as u64 {
            return None;
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, file_size, 512)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);

        let offset = file_size.checked_sub(tail_len as u64)?;
        if offset % 512 != 0 {
            return None;
        }

        let mut data = alloc::vec![0u8; tail_len];
        vbio.read_blocks(vbio.media_id(), offset / 512, &mut data)
            .ok()?;
        Some(data)
    }

    fn read_image_bytes(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        offset: u64,
        len: usize,
    ) -> Option<Vec<u8>> {
        if extents.is_empty() || len == 0 {
            return None;
        }

        let end = offset.checked_add(len as u64)?;
        if end > file_size {
            return None;
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, file_size, 512)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);

        let mut data = alloc::vec![0u8; len];
        vbio.read_bytes(vbio.media_id(), offset, &mut data).ok()?;
        Some(data)
    }

    fn resolve_iso_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        size: u64,
        extents: &[IsoExtent],
    ) -> (Option<IsoBootInfo>, bool) {
        if extents.is_empty() || size == 0 {
            return (None, false);
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, size, 2048)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();

        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        let Some(reader) = UefiBlockIo::new(block_io) else {
            return (None, false);
        };
        vbio.set_physical_reader(reader);
        let iso_io = VirtualIsoBlockIo::new(vbio);

        let boot_info = read_efi_eltorito_boot_info(&iso_io)
            .ok()
            .flatten()
            .map(IsoBootInfo::from);
        let is_udf = detect_udf_volume(&iso_io).unwrap_or(false);

        (boot_info, is_udf)
    }
}

fn block_io_info(block_io: &BlockIO) -> Option<VolumeBlockInfo> {
    let media = block_io.media();
    if !media.is_media_present() {
        return None;
    }

    let block_size = media.block_size();
    if block_size == 0 {
        return None;
    }
    let total_blocks = media.last_block().checked_add(1)?;
    let total_size = total_blocks.checked_mul(u64::from(block_size))?;

    Some(VolumeBlockInfo {
        block_size,
        total_size,
    })
}

fn source_file_extents_from_detected_fs(
    shared: nextboot_fs::SharedBlockIo,
    fs_type: FileSystemType,
    path: &str,
) -> Option<(u32, Vec<FileExtent>)> {
    match fs_type {
        FileSystemType::Fat32 => Fat32::open(shared)
            .and_then(|fs| {
                let block_size = fs.block_size();
                fs.file_extents(path).map(|extents| (block_size, extents))
            })
            .ok(),
        FileSystemType::ExFat => ExFat::open(shared)
            .and_then(|fs| {
                let block_size = fs.block_size();
                fs.file_extents(path).map(|extents| (block_size, extents))
            })
            .ok(),
        FileSystemType::Ntfs => Ntfs::open(shared)
            .and_then(|fs| {
                let block_size = fs.block_size();
                fs.file_extents(path).map(|extents| (block_size, extents))
            })
            .ok(),
        _ => Udf::open(shared.clone())
            .and_then(|fs| {
                let block_size = fs.block_size();
                fs.file_extents(path).map(|extents| (block_size, extents))
            })
            .or_else(|_| {
                Iso9660::open(shared).and_then(|fs| {
                    let block_size = fs.block_size();
                    fs.file_extents(path).map(|extents| (block_size, extents))
                })
            })
            .ok(),
    }
}

fn discover_partition_candidates(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Vec<PartitionCandidate> {
    if let Some(partitions) = discover_gpt_partitions(shared.clone(), first_block) {
        return partitions;
    }
    discover_mbr_partitions(shared, first_block)
}

fn discover_gpt_partitions(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Option<Vec<PartitionCandidate>> {
    if !has_mbr_signature(first_block) {
        return None;
    }
    let has_protective = (0..4).any(|index| {
        first_block
            .get(0x1be + index * 16 + 4)
            .copied()
            .unwrap_or(0)
            == 0xee
    });
    if !has_protective {
        return None;
    }

    let header_block = read_one_block(&shared, GPT_HEADER_LBA)?;
    let header = header_block.as_slice();
    if header.get(0..8)? != GPT_SIGNATURE {
        return None;
    }

    let header_size = read_le_u32(header, 12)?;
    if header_size < GPT_HEADER_MIN_SIZE
        || usize::try_from(header_size)
            .ok()
            .map_or(true, |len| len > header.len())
    {
        return None;
    }
    let entry_lba = read_le_u64(header, GPT_PARTITION_ENTRY_LBA_OFFSET)?;
    let num_entries = read_le_u32(header, GPT_NUM_PARTITION_ENTRIES_OFFSET)?;
    let entry_size = read_le_u32(header, GPT_PARTITION_ENTRY_SIZE_OFFSET)?;
    let entry_size = usize::try_from(entry_size).ok()?;
    if !(GPT_MIN_PARTITION_ENTRY_SIZE..=GPT_MAX_PARTITION_ENTRY_SIZE).contains(&entry_size) {
        return None;
    }

    let num_entries = usize::try_from(num_entries)
        .ok()?
        .min(GPT_MAX_PARTITION_ENTRIES);
    let entry_bytes_len = num_entries.checked_mul(entry_size)?;
    if entry_bytes_len == 0 || entry_bytes_len > GPT_MAX_PARTITION_ENTRY_ARRAY_BYTES {
        return None;
    }
    let entry_bytes = read_block_range(&shared, entry_lba, entry_bytes_len)?;

    let mut out = Vec::new();
    for index in 0..num_entries {
        let offset = index.checked_mul(entry_size)?;
        let entry = match entry_bytes.get(offset..offset + entry_size) {
            Some(entry) => entry,
            None => break,
        };
        if entry.get(0..16)?.iter().all(|byte| *byte == 0) {
            continue;
        }
        let start_lba = read_le_u64(entry, 32)?;
        let end_lba = read_le_u64(entry, 40)?;
        if start_lba == 0 || end_lba < start_lba {
            continue;
        }
        if out.try_reserve_exact(1).is_err() {
            break;
        }
        out.push(PartitionCandidate {
            number: u32::try_from(index + 1).ok()?,
            start_lba,
            block_count: end_lba - start_lba + 1,
            format: PartitionFormat::Gpt,
        });
    }

    Some(out)
}

fn discover_mbr_partitions(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Vec<PartitionCandidate> {
    let mut out = Vec::new();
    if !has_mbr_signature(first_block) {
        return out;
    }

    let mut extended_ranges = Vec::new();
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(first_block, index) else {
            continue;
        };
        if partition.partition_type == 0xee
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }

        if is_extended_mbr_partition(partition.partition_type) {
            if extended_ranges.try_reserve_exact(1).is_err() {
                continue;
            }
            extended_ranges.push(PartitionRange {
                start_lba: u64::from(partition.start_lba),
                block_count: u64::from(partition.total_sectors),
            });
            continue;
        }

        if !push_mbr_partition_candidate(
            &mut out,
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            u64::from(partition.start_lba),
            u64::from(partition.total_sectors),
        ) {
            break;
        }
    }

    let mut logical_number = MBR_LOGICAL_PARTITION_NUMBER_BASE;
    for extended in extended_ranges {
        discover_mbr_logical_partitions(&shared, extended, &mut out, &mut logical_number);
    }

    out
}

fn discover_mbr_logical_partitions(
    shared: &nextboot_fs::SharedBlockIo,
    extended: PartitionRange,
    out: &mut Vec<PartitionCandidate>,
    logical_number: &mut u32,
) {
    if extended.block_count == 0 || !range_contains_lba(extended, extended.start_lba) {
        return;
    }

    let mut visited = Vec::new();
    let mut current_ebr_lba = extended.start_lba;

    for _ in 0..MBR_MAX_LOGICAL_PARTITIONS {
        if current_ebr_lba >= shared.total_blocks()
            || !range_contains_lba(extended, current_ebr_lba)
            || visited.iter().any(|lba| *lba == current_ebr_lba)
        {
            break;
        }
        if visited.try_reserve_exact(1).is_err() {
            break;
        }
        visited.push(current_ebr_lba);

        let Some(ebr) = read_one_block(shared, current_ebr_lba) else {
            break;
        };
        if !has_mbr_signature(&ebr) {
            break;
        }

        if let Some(logical) = find_logical_mbr_partition(&ebr) {
            if let Some(start_lba) = current_ebr_lba.checked_add(u64::from(logical.start_lba)) {
                let block_count = u64::from(logical.total_sectors);
                if range_contains_extent(extended, start_lba, block_count)
                    && range_fits_disk(start_lba, block_count, shared.total_blocks())
                {
                    if !push_mbr_partition_candidate(out, *logical_number, start_lba, block_count) {
                        return;
                    }
                    *logical_number = (*logical_number).saturating_add(1);
                }
            }
        }

        let Some(next_ebr_lba) =
            find_next_ebr_lba(&ebr, extended, current_ebr_lba, shared.total_blocks())
        else {
            break;
        };
        current_ebr_lba = next_ebr_lba;
    }
}

fn find_logical_mbr_partition(block: &[u8]) -> Option<MbrPartitionEntry> {
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(block, index) else {
            continue;
        };
        if partition.partition_type == 0xee
            || is_extended_mbr_partition(partition.partition_type)
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }
        return Some(partition);
    }
    None
}

fn find_next_ebr_lba(
    block: &[u8],
    extended: PartitionRange,
    current_ebr_lba: u64,
    total_blocks: u64,
) -> Option<u64> {
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(block, index) else {
            continue;
        };
        if !is_extended_mbr_partition(partition.partition_type)
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }
        let next_ebr_lba = extended
            .start_lba
            .checked_add(u64::from(partition.start_lba))?;
        if next_ebr_lba == current_ebr_lba
            || next_ebr_lba >= total_blocks
            || !range_contains_lba(extended, next_ebr_lba)
        {
            continue;
        }
        return Some(next_ebr_lba);
    }
    None
}

fn parse_mbr_partition(block: &[u8], index: usize) -> Option<MbrPartitionEntry> {
    if index >= MBR_PRIMARY_PARTITION_COUNT || !has_mbr_signature(block) {
        return None;
    }
    let offset =
        MBR_PARTITION_TABLE_OFFSET.checked_add(index.checked_mul(MBR_PARTITION_ENTRY_SIZE)?)?;
    let partition_type = block.get(offset + 4).copied()?;
    if partition_type == 0 {
        return None;
    }
    Some(MbrPartitionEntry {
        partition_type,
        start_lba: read_le_u32(block, offset + 8)?,
        total_sectors: read_le_u32(block, offset + 12)?,
    })
}

fn push_mbr_partition_candidate(
    out: &mut Vec<PartitionCandidate>,
    number: u32,
    start_lba: u64,
    block_count: u64,
) -> bool {
    if out.try_reserve_exact(1).is_err() {
        return false;
    }
    out.push(PartitionCandidate {
        number,
        start_lba,
        block_count,
        format: PartitionFormat::Mbr,
    });
    true
}

fn read_one_block(shared: &nextboot_fs::SharedBlockIo, lba: u64) -> Option<Vec<u8>> {
    if lba >= shared.total_blocks() {
        return None;
    }
    let mut bytes = alloc_buffer_for_block(shared.block_size()).ok()?;
    shared.read_blocks(lba, &mut bytes).ok()?;
    Some(bytes)
}

fn is_extended_mbr_partition(partition_type: u8) -> bool {
    matches!(partition_type, 0x05 | 0x0f | 0x85)
}

fn range_contains_lba(range: PartitionRange, lba: u64) -> bool {
    range.block_count != 0 && lba >= range.start_lba && lba - range.start_lba < range.block_count
}

fn range_contains_extent(range: PartitionRange, start_lba: u64, block_count: u64) -> bool {
    if block_count == 0 || start_lba < range.start_lba {
        return false;
    }
    let Some(end_lba) = start_lba.checked_add(block_count) else {
        return false;
    };
    let Some(range_end_lba) = range.start_lba.checked_add(range.block_count) else {
        return false;
    };
    end_lba <= range_end_lba
}

fn range_fits_disk(start_lba: u64, block_count: u64, total_blocks: u64) -> bool {
    block_count != 0
        && start_lba
            .checked_add(block_count)
            .map_or(false, |end_lba| end_lba <= total_blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemoryBlockIo {
        block_size: usize,
        bytes: Vec<u8>,
    }

    impl MemoryBlockIo {
        fn new(block_count: usize) -> Self {
            Self::with_block_size(block_count, 512)
        }

        fn with_block_size(block_count: usize, block_size: usize) -> Self {
            let mut bytes = Vec::new();
            bytes.resize(block_count * block_size, 0);
            Self { block_size, bytes }
        }

        fn block(&self, lba: usize) -> &[u8] {
            let start = lba * self.block_size;
            &self.bytes[start..start + self.block_size]
        }

        fn block_mut(&mut self, lba: usize) -> &mut [u8] {
            let start = lba * self.block_size;
            &mut self.bytes[start..start + self.block_size]
        }
    }

    impl BlockIoOps for MemoryBlockIo {
        fn block_size(&self) -> u32 {
            self.block_size as u32
        }

        fn total_blocks(&self) -> u64 {
            (self.bytes.len() / self.block_size) as u64
        }

        fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            if buf.is_empty() || buf.len() % self.block_size != 0 {
                return Err(FsError::InvalidArgument);
            }
            let start = usize::try_from(lba).map_err(|_| FsError::ReadError)?;
            let block_count = buf.len() / self.block_size;
            let end_block = start.checked_add(block_count).ok_or(FsError::ReadError)?;
            let byte_start = start
                .checked_mul(self.block_size)
                .ok_or(FsError::ReadError)?;
            let byte_end = end_block
                .checked_mul(self.block_size)
                .ok_or(FsError::ReadError)?;
            let bytes = self
                .bytes
                .get(byte_start..byte_end)
                .ok_or(FsError::ReadError)?;
            buf.copy_from_slice(bytes);
            Ok(())
        }
    }

    fn write_mbr_entry(
        block: &mut [u8],
        index: usize,
        partition_type: u8,
        start_lba: u32,
        total_sectors: u32,
    ) {
        block[510] = 0x55;
        block[511] = 0xaa;
        let offset = MBR_PARTITION_TABLE_OFFSET + index * MBR_PARTITION_ENTRY_SIZE;
        block[offset + 4] = partition_type;
        block[offset + 8..offset + 12].copy_from_slice(&start_lba.to_le_bytes());
        block[offset + 12..offset + 16].copy_from_slice(&total_sectors.to_le_bytes());
    }

    fn write_protective_mbr(block: &mut [u8]) {
        write_mbr_entry(block, 0, 0xee, 1, u32::MAX);
    }

    fn write_gpt_header(block: &mut [u8], entry_lba: u64, num_entries: u32, entry_size: u32) {
        block[0..8].copy_from_slice(GPT_SIGNATURE);
        block[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        block[12..16].copy_from_slice(&GPT_HEADER_MIN_SIZE.to_le_bytes());
        block[24..32].copy_from_slice(&GPT_HEADER_LBA.to_le_bytes());
        block[72..80].copy_from_slice(&entry_lba.to_le_bytes());
        block[80..84].copy_from_slice(&num_entries.to_le_bytes());
        block[84..88].copy_from_slice(&entry_size.to_le_bytes());
    }

    fn write_gpt_entry(block: &mut [u8], offset: usize, start_lba: u64, end_lba: u64) {
        let entry = &mut block[offset..offset + GPT_MIN_PARTITION_ENTRY_SIZE];
        entry[0] = 1;
        entry[16] = 2;
        entry[32..40].copy_from_slice(&start_lba.to_le_bytes());
        entry[40..48].copy_from_slice(&end_lba.to_le_bytes());
    }

    #[test]
    fn discovers_mbr_logical_partitions_from_ebr_chain() {
        let mut disk = MemoryBlockIo::new(32_000);
        write_mbr_entry(disk.block_mut(0), 1, 0x07, 2048, 4096);
        write_mbr_entry(disk.block_mut(0), 2, 0x0f, 10_000, 10_000);
        write_mbr_entry(disk.block_mut(10_000), 0, 0x07, 63, 1000);
        write_mbr_entry(disk.block_mut(10_000), 1, 0x05, 2000, 8000);
        write_mbr_entry(disk.block_mut(12_000), 0, 0x0b, 128, 500);

        let first_block = disk.block(0).to_vec();
        let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
        let partitions = discover_mbr_partitions(shared, &first_block);

        assert_eq!(partitions.len(), 3);
        assert_eq!(partitions[0].number, 2);
        assert_eq!(partitions[0].start_lba, 2048);
        assert_eq!(partitions[0].block_count, 4096);
        assert_eq!(partitions[0].format, PartitionFormat::Mbr);
        assert_eq!(partitions[1].number, 5);
        assert_eq!(partitions[1].start_lba, 10_063);
        assert_eq!(partitions[1].block_count, 1000);
        assert_eq!(partitions[2].number, 6);
        assert_eq!(partitions[2].start_lba, 12_128);
        assert_eq!(partitions[2].block_count, 500);
    }

    #[test]
    fn discovers_gpt_partitions_from_entry_array_beyond_prefix_window() {
        let mut disk = MemoryBlockIo::new(2048);
        let entry_lba = 600;
        write_protective_mbr(disk.block_mut(0));
        write_gpt_header(
            disk.block_mut(1),
            entry_lba,
            2,
            GPT_MIN_PARTITION_ENTRY_SIZE as u32,
        );
        write_gpt_entry(disk.block_mut(entry_lba as usize), 0, 700, 799);
        write_gpt_entry(
            disk.block_mut(entry_lba as usize),
            GPT_MIN_PARTITION_ENTRY_SIZE,
            1024,
            1535,
        );

        let first_block = disk.block(0).to_vec();
        let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
        let partitions = discover_gpt_partitions(shared, &first_block).expect("gpt partitions");

        assert_eq!(partitions.len(), 2);
        assert_eq!(partitions[0].number, 1);
        assert_eq!(partitions[0].start_lba, 700);
        assert_eq!(partitions[0].block_count, 100);
        assert_eq!(partitions[0].format, PartitionFormat::Gpt);
        assert_eq!(partitions[1].number, 2);
        assert_eq!(partitions[1].start_lba, 1024);
        assert_eq!(partitions[1].block_count, 512);
    }

    #[test]
    fn discovers_gpt_partitions_on_4k_native_disk() {
        let mut disk = MemoryBlockIo::with_block_size(128, 4096);
        write_protective_mbr(disk.block_mut(0));
        write_gpt_header(disk.block_mut(1), 2, 1, GPT_MIN_PARTITION_ENTRY_SIZE as u32);
        write_gpt_entry(disk.block_mut(2), 0, 16, 63);

        let first_block = disk.block(0).to_vec();
        let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
        let partitions = discover_gpt_partitions(shared, &first_block).expect("gpt partitions");

        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].number, 1);
        assert_eq!(partitions[0].start_lba, 16);
        assert_eq!(partitions[0].block_count, 48);
        assert_eq!(partitions[0].format, PartitionFormat::Gpt);
    }

    #[test]
    fn treats_ventoy_plugin_directory_as_non_image_tree() {
        assert!(is_ventoy_plugin_tree_path("/ventoy"));
        assert!(is_ventoy_plugin_tree_path("/Ventoy/dud/dd.iso"));
        assert!(!is_ventoy_plugin_tree_path("/ISO/ventoy-linux.iso"));
        assert!(!is_ventoy_plugin_tree_path("/persistence/ventoy.dat"));
    }

    #[test]
    fn treats_default_uefi_bootloader_paths_as_non_images() {
        assert!(is_default_uefi_bootloader_path("/EFI/BOOT/BOOTX64.EFI"));
        assert!(is_default_uefi_bootloader_path("/efi/boot/bootaa64.efi"));
        assert!(!is_default_uefi_bootloader_path("/ISO/tools.efi"));
        assert!(!is_default_uefi_bootloader_path("/EFI/tools.efi"));
    }
}

fn has_mbr_signature(block: &[u8]) -> bool {
    block.get(510) == Some(&0x55) && block.get(511) == Some(&0xaa)
}

fn partition_source_disk_identity(
    first_block: &[u8],
    volume_info: VolumeBlockInfo,
    partition: PartitionCandidate,
) -> Option<SourceDiskIdentity> {
    let info = HardDriveDevicePathInfo {
        node_offset: 0,
        partition_number: partition.number,
        partition_start_lba: partition.start_lba,
        partition_size_blocks: partition.block_count,
        partition_format: partition.format,
        signature_type: match partition.format {
            PartitionFormat::Gpt => 2,
            PartitionFormat::Mbr => 1,
            PartitionFormat::Unknown => 0,
        },
    };
    build_source_disk_identity(
        first_block,
        volume_info.total_size,
        volume_info.block_size,
        Some(info),
    )
}

fn read_block_range(
    shared: &nextboot_fs::SharedBlockIo,
    start_lba: u64,
    byte_len: usize,
) -> Option<Vec<u8>> {
    if byte_len == 0 {
        return Some(Vec::new());
    }

    let block_size = usize::try_from(shared.block_size()).ok()?;
    if block_size == 0 {
        return None;
    }
    let block_count = div_round_up_usize(byte_len, block_size);
    let block_count_u64 = u64::try_from(block_count).ok()?;
    if start_lba
        .checked_add(block_count_u64)
        .map_or(true, |end| end > shared.total_blocks())
    {
        return None;
    }

    let len = block_count.checked_mul(block_size)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).ok()?;
    bytes.resize(len, 0);
    shared.read_blocks(start_lba, &mut bytes).ok()?;
    bytes.truncate(byte_len);
    Some(bytes)
}

fn read_uefi_regular_file(
    parent: &mut Directory,
    name: &str,
    expected_size: u64,
) -> uefi::Result<Vec<u8>> {
    if expected_size != vlnk::VLNK_FILE_LEN as u64 {
        return Err(Status::INVALID_PARAMETER.into());
    }
    let file_size = usize::try_from(expected_size).map_err(|_| Status::OUT_OF_RESOURCES)?;
    let c_path = CString16::try_from(name).map_err(|_| Status::INVALID_PARAMETER)?;
    let handle = parent.open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())?;
    let mut file = handle
        .into_regular_file()
        .ok_or_else(|| uefi::Error::new(Status::NOT_FOUND, ()))?;
    let mut data = Vec::new();
    data.try_reserve_exact(file_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    data.resize(file_size, 0);

    let mut offset = 0usize;
    while offset < data.len() {
        let read = file.read(&mut data[offset..])?;
        if read == 0 {
            break;
        }
        offset = offset
            .checked_add(read)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
    }
    data.truncate(offset);
    Ok(data)
}

fn vlnk_matches_source_disk(source_disk: Option<SourceDiskIdentity>, vlnk: &VentoyVlnk) -> bool {
    let Some(disk) = source_disk else {
        return false;
    };
    if disk.disk_signature != vlnk.disk_signature {
        return false;
    }
    partition_offset_matches(
        disk.partition_start_lba,
        disk.block_size,
        vlnk.part_offset_bytes,
    )
}

fn vlnk_matches_partition(
    partition: PartitionCandidate,
    block_size: u32,
    vlnk: &VentoyVlnk,
) -> bool {
    partition_offset_matches(partition.start_lba, block_size, vlnk.part_offset_bytes)
}

fn partition_offset_matches(start_lba: u64, block_size: u32, expected_bytes: u64) -> bool {
    let native = start_lba
        .checked_mul(u64::from(block_size))
        .is_some_and(|offset| offset == expected_bytes);
    let ventoy_sector = start_lba
        .checked_mul(512)
        .is_some_and(|offset| offset == expected_bytes);
    native || ventoy_sector
}

fn normalize_vlnk_target_path(path: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for ch in path.trim().chars() {
        let ch = if ch == '\\' { '/' } else { ch };
        if ch == '/' {
            if previous_was_separator {
                continue;
            }
            previous_was_separator = true;
        } else {
            previous_was_separator = false;
        }
        normalized.push(ch);
    }
    if normalized.is_empty() {
        return String::from("/");
    }
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    normalized
}

fn offset_extents_for_physical_read(
    extents: &[IsoExtent],
    lba_offset: u64,
) -> Option<Vec<IsoExtent>> {
    if lba_offset == 0 {
        return Some(extents.to_vec());
    }

    let mut out = Vec::new();
    out.try_reserve_exact(extents.len()).ok()?;
    for extent in extents {
        out.push(IsoExtent {
            virtual_block_start: extent.virtual_block_start,
            physical_lba: extent.physical_lba.checked_add(lba_offset)?,
            block_count: extent.block_count,
        });
    }
    Some(out)
}

fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_le_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn div_round_up_usize(value: usize, divisor: usize) -> usize {
    if divisor == 0 {
        0
    } else {
        value.saturating_add(divisor - 1) / divisor
    }
}

fn alloc_buffer_for_block(block_size: u32) -> Result<Vec<u8>, FsError> {
    let len = usize::try_from(block_size).map_err(|_| FsError::InvalidArgument)?;
    if len == 0 {
        return Err(FsError::InvalidArgument);
    }

    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|_| FsError::OutOfMemory)?;
    buf.resize(len, 0);
    Ok(buf)
}

fn handle_list_contains(handles: &[Handle], needle: Handle) -> bool {
    handles
        .iter()
        .any(|handle| handle.as_ptr() == needle.as_ptr())
}

fn should_descend_into_directory(depth: usize, max_search_level: Option<usize>) -> bool {
    max_search_level.map_or(true, |max_depth| depth < max_depth)
}

fn device_path_to_vec(device_path: &DevicePath) -> Option<Vec<u8>> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Some(bytes.to_vec())
}

unsafe fn device_path_byte_len(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }

    let mut offset = 0usize;
    loop {
        let node = unsafe { ptr.add(offset) };
        let node_type = unsafe { ptr::read_unaligned(node) };
        let node_subtype = unsafe { ptr::read_unaligned(node.add(1)) };
        let len_lo = unsafe { ptr::read_unaligned(node.add(2)) };
        let len_hi = unsafe { ptr::read_unaligned(node.add(3)) };
        let node_len = u16::from_le_bytes([len_lo, len_hi]) as usize;
        if node_len < 4 {
            return None;
        }

        offset = offset.checked_add(node_len)?;
        if node_type == 0x7f && node_subtype == 0xff {
            return Some(offset);
        }
    }
}

struct UefiBlockIo {
    block_io: NonNull<BlockIO>,
    media_id: u32,
    block_size: u32,
    total_blocks: u64,
}

impl UefiBlockIo {
    fn new(block_io: &BlockIO) -> Option<Self> {
        let media = block_io.media();
        let block_size = media.block_size();
        if block_size == 0 {
            return None;
        }

        Some(Self {
            block_io: NonNull::from(block_io),
            media_id: media.media_id(),
            block_size,
            total_blocks: media.last_block() + 1,
        })
    }
}

impl BlockIoOps for UefiBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(FsError::InvalidArgument);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(FsError::ReadError);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| FsError::ReadError)
    }
}

struct PartitionBlockIo {
    parent: nextboot_fs::SharedBlockIo,
    start_lba: u64,
    total_blocks: u64,
}

impl PartitionBlockIo {
    fn new(parent: nextboot_fs::SharedBlockIo, start_lba: u64, total_blocks: u64) -> Self {
        Self {
            parent,
            start_lba,
            total_blocks,
        }
    }
}

impl BlockIoOps for PartitionBlockIo {
    fn block_size(&self) -> u32 {
        self.parent.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size() as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(FsError::InvalidArgument);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(FsError::ReadError);
        }

        let parent_lba = self.start_lba.checked_add(lba).ok_or(FsError::ReadError)?;
        self.parent.read_blocks(parent_lba, buf)
    }
}

impl PhysicalReader for UefiBlockIo {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(VirtIoError::OutOfBounds);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| VirtIoError::ReadFailed)
    }
}

struct VirtualIsoBlockIo {
    vbio: VirtualBlockIo,
    media_id: u32,
}

impl VirtualIsoBlockIo {
    fn new(vbio: VirtualBlockIo) -> Self {
        let media_id = vbio.media_id();
        Self { vbio, media_id }
    }
}

impl BlockIoOps for VirtualIsoBlockIo {
    fn block_size(&self) -> u32 {
        self.vbio.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.vbio.block_count()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.vbio
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| FsError::ReadError)
    }
}

fn open_directory(parent: &mut Directory, path: &str) -> uefi::Result<Directory> {
    let uefi_path = to_uefi_relative_path(path);
    let c_path =
        CString16::try_from(uefi_path.as_str()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let handle = parent.open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())?;
    handle
        .into_directory()
        .ok_or_else(|| uefi::Error::new(uefi::Status::NOT_FOUND, ()))
}

fn normalize_scan_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return String::from("/");
    }

    let mut normalized = String::from("/");
    normalized.push_str(trimmed.trim_matches('/'));
    normalized
}

fn to_uefi_relative_path(path: &str) -> String {
    let mut out = String::new();
    for (index, part) in path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        if index > 0 {
            out.push('\\');
        }
        out.push_str(part);
    }
    out
}

fn join_display_path(parent: &str, name: &str) -> String {
    if parent == "/" || parent.is_empty() {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn cstr16_to_string(name: &uefi::CStr16) -> String {
    let mut out = String::new();
    for ch in name.as_slice() {
        let c = char::from(*ch);
        if c == '\0' {
            break;
        }
        out.push(c);
    }
    out
}

fn has_supported_extension(name: &str, extensions: &[&str]) -> bool {
    let lower = name.to_lowercase();
    extensions.iter().any(|ext| lower.ends_with(ext))
}

#[derive(Debug, Clone, Copy)]
struct VhdFooterInfo {
    image_format: ImageFormat,
    virtual_size: u64,
}

fn parse_vhd_footer(footer: &[u8]) -> Option<VhdFooterInfo> {
    if footer.len() < 512 || footer.get(0..8)? != b"conectix" {
        return None;
    }

    let virtual_size = u64::from_be_bytes(footer.get(48..56)?.try_into().ok()?);
    let disk_type = u32::from_be_bytes(footer.get(60..64)?.try_into().ok()?);
    if virtual_size == 0 {
        return None;
    }

    Some(VhdFooterInfo {
        image_format: ImageFormat::from_vhd_disk_type(disk_type),
        virtual_size,
    })
}

fn default_virtual_block_size(image_format: ImageFormat) -> Option<u32> {
    if image_format.uses_512_byte_virtual_sectors() {
        Some(512)
    } else {
        None
    }
}

fn is_hidden_tree(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | ".Trash" | ".Spotlight-V100" | ".fseventsd"
    )
}

fn is_ventoy_plugin_tree_path(path: &str) -> bool {
    path.trim_matches('/')
        .split('/')
        .next()
        .is_some_and(|part| part.eq_ignore_ascii_case("ventoy"))
}

fn is_default_uefi_bootloader_path(path: &str) -> bool {
    let mut parts = path.trim_matches('/').split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(filename) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if !first.eq_ignore_ascii_case("efi") || !second.eq_ignore_ascii_case("boot") {
        return false;
    }

    filename.eq_ignore_ascii_case("bootx64.efi")
        || filename.eq_ignore_ascii_case("bootaa64.efi")
        || filename.eq_ignore_ascii_case("bootia32.efi")
        || filename.eq_ignore_ascii_case("bootarm.efi")
}

fn is_dot_underscore_file(name: &str) -> bool {
    name.starts_with("._")
}

/// 缓存的 ISO 列表
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
    pub fn is_valid(&self, max_age_seconds: u64) -> bool {
        // TODO: 检查时间戳
        !self.entries.is_empty()
    }
}

impl Default for IsoCache {
    fn default() -> Self {
        Self::new()
    }
}
