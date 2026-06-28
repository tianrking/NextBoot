//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

mod block_devices;
mod block_io;
mod block_paths;
mod common;
mod config;
mod exposed_partitions;
mod helpers;
mod image_metadata;
mod metadata;
mod model;
mod partitions;
mod paths;
mod source_extents;
mod uefi_paths;
mod vlnk_filesystems;
mod vlnk_links;

use crate::ventoy_config::VentoyConfig;
use alloc::vec::Vec;
use common::{
    block_io_info, device_path_to_vec, handle_list_contains, normalize_vlnk_target_path,
    partition_source_disk_identity, read_uefi_regular_file, should_descend_into_directory,
    vlnk_matches_partition, vlnk_matches_source_disk,
};
use paths::{is_ventoy_plugin_tree_path, normalize_scan_path, open_directory};
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
        os_type_hint: Option<OsType>,
    ) -> OsType {
        if image_format.is_wim_container() {
            if let Some(info) = wim_info {
                if info.is_bootable() {
                    return OsType::WinPE;
                }
            }
        }

        if let Some(os_type) = os_type_hint {
            return os_type;
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
}
