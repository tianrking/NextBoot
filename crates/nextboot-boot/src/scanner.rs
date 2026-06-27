//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

mod block_devices;
mod block_io;
mod config;
mod helpers;
mod metadata;
mod model;
mod partitions;
mod paths;
mod vlnk_links;

use crate::source_disk::{
    build_source_disk_identity, HardDriveDevicePathInfo, PartitionFormat, SourceDiskIdentity,
};
use crate::ventoy_config::VentoyConfig;
use crate::vlnk::{self, VentoyVlnk};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr;
use helpers::default_virtual_block_size;
use model::{PartitionCandidate, ResolvedImageMetadata, VolumeBlockInfo};
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::{FileExtent, FileSystem, FileSystemType, FsError};
use paths::{
    cstr16_to_string, has_supported_extension, is_default_uefi_bootloader_path,
    is_dot_underscore_file, is_hidden_tree, is_ventoy_plugin_tree_path, join_display_path,
    normalize_scan_path, open_directory,
};
use uefi::data_types::CString16;
use uefi::proto::device_path::DevicePath;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::{Handle, Identify, Status};

pub use model::{ImageFormat, IsoExtent, IsoFile, OsType, WimBootInfo};
#[allow(unused_imports)]
pub use model::{IsoBootInfo, IsoCache, WimCompression};

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
