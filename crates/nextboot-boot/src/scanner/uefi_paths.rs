use super::helpers::default_virtual_block_size;
use super::model::{ImageFormat, IsoFile, ResolvedImageMetadata};
use super::paths::{
    cstr16_to_string, has_supported_extension, is_default_uefi_bootloader_path,
    is_dot_underscore_file, is_hidden_tree, is_ventoy_plugin_tree_path, join_display_path,
    open_directory,
};
use super::{should_descend_into_directory, IsoScanner};
use crate::source_disk::SourceDiskIdentity;
use crate::ventoy_config::VentoyConfig;
use crate::vlnk;
use alloc::string::ToString;
use alloc::vec::Vec;
use uefi::proto::media::file::Directory;
use uefi::Handle;

impl<'a> IsoScanner<'a> {
    pub(super) fn scan_directory_entries(
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
                    os_type_hint,
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
                        os_type_hint: None,
                        image_format,
                        virtual_size: entry.file_size(),
                        virtual_block_size: default_virtual_block_size(image_format),
                    });
                let start_lba = extents.first().map_or(0, |extent| extent.physical_lba);
                let os_type =
                    self.detect_image_os_type(&full_path, image_format, wim_info, os_type_hint);

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
