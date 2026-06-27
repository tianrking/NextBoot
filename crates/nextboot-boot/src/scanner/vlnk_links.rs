use super::block_io::{alloc_buffer_for_block, PartitionBlockIo, UefiBlockIo};
use super::model::{ImageFormat, IsoFile};
use super::{
    block_io_info, discover_partition_candidates, normalize_vlnk_target_path,
    partition_source_disk_identity, read_uefi_regular_file, vlnk_matches_partition,
    vlnk_matches_source_disk, IsoScanner,
};
use crate::source_disk::SourceDiskIdentity;
use crate::ventoy_config::VentoyConfig;
use crate::vlnk::{self, VentoyVlnk};
use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec::Vec;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::{detect_fs_type, FileSystem, FileSystemType};
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::Directory;
use uefi::table::boot::SearchType;
use uefi::{Handle, Identify};

impl<'a> IsoScanner<'a> {
    pub(super) fn resolve_uefi_vlnk_file(
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

    pub(super) fn resolve_block_vlnk_file<F: FileSystem>(
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
}
