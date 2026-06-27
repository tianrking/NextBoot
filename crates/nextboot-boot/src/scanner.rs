//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use crate::init::StorageDevice;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::NonNull;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::{read_efi_eltorito_boot_info, ElToritoBootInfo};
use nextboot_fs::{detect_fs_type, BlockIoOps, FileExtent, FileSystem, FileSystemType, FsError};
use nextboot_virtio::{
    PhysicalReader, VirtIoError, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType,
};
use uefi::data_types::CString16;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::{Handle, Identify};

const MAX_SCAN_DEPTH: usize = 4;

/// ISO 文件信息
#[derive(Debug, Clone)]
pub struct IsoFile {
    /// 文件路径
    pub path: String,
    /// 文件大小 (字节)
    pub size: u64,
    /// 启动时呈现给固件/系统的虚拟介质大小
    pub virtual_size: u64,
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

impl From<FileExtent> for IsoExtent {
    fn from(extent: FileExtent) -> Self {
        Self {
            virtual_block_start: extent.virtual_block_start,
            physical_lba: extent.physical_lba,
            block_count: extent.block_count,
        }
    }
}

/// 可启动镜像格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Iso,
    Wim,
    Esd,
    RawDisk,
    Vhd,
    FixedVhd,
    DynamicVhd,
    DifferencingVhd,
    Vhdx,
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
        } else if lower.ends_with(".img") {
            Self::RawDisk
        } else if lower.ends_with(".vhd") {
            Self::Vhd
        } else if lower.ends_with(".vhdx") {
            Self::Vhdx
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

    pub fn supports_virtual_disk_boot(self) -> bool {
        matches!(
            self,
            Self::Iso | Self::RawDisk | Self::FixedVhd | Self::DynamicVhd
        )
    }

    pub fn uses_512_byte_virtual_sectors(self) -> bool {
        matches!(self, Self::RawDisk | Self::FixedVhd | Self::DynamicVhd)
    }
}

impl core::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self {
            ImageFormat::Iso => "ISO",
            ImageFormat::Wim => "WIM",
            ImageFormat::Esd => "ESD",
            ImageFormat::RawDisk => "RAW",
            ImageFormat::Vhd => "VHD",
            ImageFormat::FixedVhd => "Fixed VHD",
            ImageFormat::DynamicVhd => "Dynamic VHD",
            ImageFormat::DifferencingVhd => "Differencing VHD",
            ImageFormat::Vhdx => "VHDX",
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
        let extensions = [".iso", ".wim", ".img", ".vhd", ".vhdx", ".esd"];

        // 扫描常见目录
        let search_paths = [
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

            for search_path in &search_paths {
                if let Ok(files) =
                    self.scan_volume_path(volume_index, handle, &mut fs, search_path, &extensions)
                {
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

            if let Ok(mut volume_files) =
                self.scan_volume_path(volume_index, handle, &mut fs, path, extensions)
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

    fn scan_volume_path(
        &self,
        volume_index: usize,
        volume_handle: Handle,
        fs: &mut SimpleFileSystem,
        path: &str,
        extensions: &[&str],
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
        self.scan_directory_entries(
            volume_handle,
            volume_index,
            &mut dir,
            &normalized,
            extensions,
            0,
            &mut files,
        )?;
        Ok(files)
    }

    fn scan_directory_entries(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        dir: &mut Directory,
        display_path: &str,
        extensions: &[&str],
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
                        &mut child,
                        &full_path,
                        extensions,
                        depth + 1,
                        files,
                    );
                }
                continue;
            }

            if has_supported_extension(&name, extensions) {
                let image_format = ImageFormat::detect_from_path(&full_path);
                let (block_size, extents, boot_info, image_format, virtual_size) = self
                    .resolve_image_metadata(
                        volume_handle,
                        &full_path,
                        entry.file_size(),
                        image_format,
                    )
                    .unwrap_or((
                        self.device.block_size,
                        Vec::new(),
                        None,
                        image_format,
                        entry.file_size(),
                    ));
                let start_lba = extents.first().map_or(0, |extent| extent.physical_lba);

                files.push(IsoFile {
                    path: full_path.clone(),
                    size: entry.file_size(),
                    virtual_size,
                    volume_handle,
                    volume_index,
                    block_size,
                    start_lba,
                    extents,
                    os_type: self.detect_iso_type(&full_path),
                    image_format,
                    boot_info,
                });
            }
        }

        Ok(())
    }

    fn resolve_image_metadata(
        &self,
        volume_handle: Handle,
        path: &str,
        size: u64,
        image_format: ImageFormat,
    ) -> Option<(u32, Vec<IsoExtent>, Option<IsoBootInfo>, ImageFormat, u64)> {
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
        let (image_format, virtual_size) =
            self.detect_image_virtual_metadata(&block_io, block_size, size, &extents, image_format);
        let boot_info = if image_format.is_iso() {
            self.resolve_iso_boot_info(&block_io, block_size, size, &extents)
        } else {
            None
        };

        Some((block_size, extents, boot_info, image_format, virtual_size))
    }

    fn detect_image_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        image_format: ImageFormat,
    ) -> (ImageFormat, u64) {
        if !matches!(image_format, ImageFormat::Vhd) {
            return (image_format, file_size);
        }

        match self.read_image_tail(block_io, source_block_size, file_size, extents, 512) {
            Some(footer) => parse_vhd_footer(&footer)
                .map(|info| {
                    let virtual_size = if info.image_format == ImageFormat::FixedVhd {
                        info.virtual_size.min(file_size.saturating_sub(512))
                    } else {
                        info.virtual_size
                    };
                    (info.image_format, virtual_size)
                })
                .unwrap_or((image_format, file_size)),
            None => (image_format, file_size),
        }
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

    fn resolve_iso_boot_info(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        size: u64,
        extents: &[IsoExtent],
    ) -> Option<IsoBootInfo> {
        if extents.is_empty() || size == 0 {
            return None;
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
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);
        let iso_io = VirtualIsoBlockIo::new(vbio);

        read_efi_eltorito_boot_info(&iso_io)
            .ok()
            .flatten()
            .map(IsoBootInfo::from)
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
