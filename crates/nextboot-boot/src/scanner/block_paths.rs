use super::helpers::default_virtual_block_size;
use super::model::{ImageFormat, IsoFile, ResolvedImageMetadata};
use super::paths::{
    has_supported_extension, is_default_uefi_bootloader_path, is_dot_underscore_file,
    is_hidden_tree, is_ventoy_plugin_tree_path, join_display_path, normalize_scan_path,
};
use super::{should_descend_into_directory, IsoScanner};
use crate::source_disk::SourceDiskIdentity;
use crate::ventoy_config::VentoyConfig;
use crate::vlnk;
use alloc::string::ToString;
use alloc::vec::Vec;
use nextboot_fs::{FileSystem, FsError};
use uefi::proto::media::block::BlockIO;
use uefi::Handle;

impl<'a> IsoScanner<'a> {
    pub(super) fn scan_block_filesystem_paths<F: FileSystem>(
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
                        os_type_hint: None,
                        image_format,
                        virtual_size: entry.size,
                        virtual_block_size: default_virtual_block_size(image_format),
                    });
                let start_lba = metadata
                    .extents
                    .first()
                    .map_or(0, |extent| extent.physical_lba);
                let os_type = self.detect_image_os_type(
                    &full_path,
                    metadata.image_format,
                    metadata.wim_info,
                    metadata.os_type_hint,
                );

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
}
