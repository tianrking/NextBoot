//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

mod block_io;
mod helpers;
mod model;
mod partitions;
mod paths;

use crate::source_disk::{
    build_source_disk_identity, parent_device_path_bytes, parse_last_hard_drive_device_path,
    HardDriveDevicePathInfo, PartitionFormat, SourceDiskIdentity,
};
use crate::vdi;
use crate::ventoy_config::{VentoyConfig, VentoyConfigError};
use crate::vhdx;
use crate::vlnk::{self, VentoyVlnk};
use crate::wim;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use block_io::{alloc_buffer_for_block, PartitionBlockIo, UefiBlockIo, VirtualIsoBlockIo};
use core::ptr;
use helpers::{default_virtual_block_size, offset_extents_for_physical_read, parse_vhd_footer};
use model::{PartitionCandidate, PartitionRange, ResolvedImageMetadata, VolumeBlockInfo};
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::{detect_udf_volume, read_efi_eltorito_boot_info, Iso9660};
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileExtent, FileSystem, FileSystemType, FsError};
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use partitions::discover_partition_candidates;
use paths::{
    cstr16_to_string, has_supported_extension, is_default_uefi_bootloader_path,
    is_dot_underscore_file, is_hidden_tree, is_ventoy_plugin_tree_path, join_display_path,
    normalize_scan_path, open_directory, to_uefi_relative_path,
};
use uefi::data_types::CString16;
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::{Handle, Identify, Status};

pub use model::{ImageFormat, IsoBootInfo, IsoExtent, IsoFile, OsType, WimBootInfo};
#[allow(unused_imports)]
pub use model::{IsoCache, WimCompression};

const VENTOY_CONFIG_PATH: &str = "/ventoy/ventoy.json";
const VENTOY_CONFIG_MAX_SIZE: usize = 256 * 1024;
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
