//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use crate::init::StorageDevice;
use crate::source_disk::{
    build_source_disk_identity, parent_device_path_bytes, parse_last_hard_drive_device_path,
    SourceDiskIdentity,
};
use crate::vdi;
use crate::ventoy_config::{VentoyConfig, VentoyConfigError};
use crate::vhdx;
use crate::wim;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr::{self, NonNull};
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::{detect_udf_volume, read_efi_eltorito_boot_info, ElToritoBootInfo};
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
use uefi::{Handle, Identify};

const MAX_SCAN_DEPTH: usize = 4;
const VENTOY_CONFIG_PATH: &str = "/ventoy/ventoy.json";
const VENTOY_CONFIG_MAX_SIZE: usize = 256 * 1024;

/// ISO 文件信息
#[derive(Debug, Clone)]
pub struct IsoFile {
    /// 文件路径
    pub path: String,
    /// Ventoy menu_alias 插件提供的显示名。
    pub menu_alias: Option<String>,
    /// 文件大小 (字节)
    pub size: u64,
    /// 启动时呈现给固件/系统的虚拟介质大小
    pub virtual_size: u64,
    /// 启动时呈现给固件/系统的虚拟逻辑块大小
    pub virtual_block_size: Option<u32>,
    /// 文件所在的 UEFI volume handle
    pub volume_handle: Handle,
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
    device: &'a StorageDevice,
}

impl<'a> IsoScanner<'a> {
    /// 创建新的扫描器
    pub fn new(bt: &'a BootServices, device: &'a StorageDevice) -> Self {
        Self { bt, device }
    }

    /// 扫描指定目录下的 ISO 文件
    pub fn scan(&self, root: &str) -> uefi::Result<Vec<IsoFile>> {
        let mut iso_files = Vec::new();

        // 支持的文件扩展名
        let extensions = [
            ".iso", ".wim", ".img", ".vhd", ".vhdx", ".vdi", ".esd", ".efi",
        ];

        // 扫描常见目录
        let default_search_paths = [
            root, "/", "/ISO", "/iso", "/Images", "/images", "/Boot", "/boot",
        ];

        let fs_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;

        for (volume_index, handle) in fs_handles.iter().copied().enumerate() {
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

        // 去重。相同卷上的相同路径可能会被多个 search path 扫到；不同卷
        // 上的同名镜像必须保留，这是固定盘/多 SSD 场景的关键差异。
        iso_files.sort_by(|a, b| {
            a.volume_index
                .cmp(&b.volume_index)
                .then_with(|| a.path.cmp(&b.path))
        });
        iso_files.dedup_by(|a, b| a.volume_index == b.volume_index && a.path == b.path);

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
        let fs_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
        let mut files = Vec::new();

        for (volume_index, handle) in fs_handles.iter().copied().enumerate() {
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
        self.scan_directory_entries(
            volume_handle,
            volume_index,
            source_disk,
            &mut dir,
            &normalized,
            extensions,
            config,
            0,
            &mut files,
        )?;
        Ok(files)
    }

    fn scan_directory_entries(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        dir: &mut Directory,
        display_path: &str,
        extensions: &[&str],
        config: &VentoyConfig,
        depth: usize,
        files: &mut Vec<IsoFile>,
    ) -> uefi::Result<()> {
        while let Some(entry) = dir.read_entry_boxed()? {
            let name = cstr16_to_string(entry.file_name());

            if name.is_empty() || name == "." || name == ".." {
                continue;
            }

            let full_path = join_display_path(display_path, &name);

            if entry.is_directory() {
                if depth >= MAX_SCAN_DEPTH || is_hidden_tree(&name) {
                    continue;
                }

                if let Ok(mut child) = open_directory(dir, &name) {
                    let _ = self.scan_directory_entries(
                        volume_handle,
                        volume_index,
                        source_disk,
                        &mut child,
                        &full_path,
                        extensions,
                        config,
                        depth + 1,
                        files,
                    );
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
                        block_size: self.device.block_size,
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
                    size: entry.file_size(),
                    virtual_size,
                    virtual_block_size,
                    volume_handle,
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
                });
            }
        }

        Ok(())
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

        let extents = match detect_fs_type(&boot_sector) {
            FileSystemType::Fat32 => Fat32::open(shared)
                .and_then(|fs| fs.file_extents(path))
                .ok()?,
            FileSystemType::ExFat => ExFat::open(shared)
                .and_then(|fs| fs.file_extents(path))
                .ok()?,
            _ => return None,
        };

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
