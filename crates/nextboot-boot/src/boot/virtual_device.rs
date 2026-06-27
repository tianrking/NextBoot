use super::candidates::{ISO9660_SECTOR_SIZE, VENTOY_CONF_REPLACE_MAX_SIZE};
use super::errors::virtio_error_to_uefi_status;
use super::load_file::PreloadedLoadFileProtocol;
use super::source_volume::SourceVolumeFileSystem;
use super::util::{align_up, align_up_u64, iso9660_file_extent_patch};
use super::{BootManager, VirtualBootDevice};
use crate::virtual_fs::VirtualFileReplacement;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_virtio::protocol::VirtualBlockIoProtocol;
use nextboot_virtio::{MemoryOverlay, VirtualDeviceConfig};
use uefi::proto::media::block::BlockIO;
use uefi::Status;

impl BootManager<'_> {
    /// 创建虚拟 Block IO
    pub(super) fn create_virtual_block_io(
        &self,
        mut config: VirtualDeviceConfig,
    ) -> uefi::Result<VirtualBootDevice> {
        info!("Creating virtual Block IO...");
        let load_file_entries = self.preload_load_file_entries();

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let memory_overlays = self.build_conf_replace_overlays(&source_block_io, &mut config)?;
        let efi_file_replacements = self.build_efi_file_replacements(&source_block_io)?;
        let mut vbio = self.build_virtual_block_io(config, &source_block_io)?;
        for overlay in memory_overlays {
            vbio.add_memory_overlay(overlay)
                .map_err(virtio_error_to_uefi_status)?;
        }
        let virtual_info = vbio.device_info();
        let registered = VirtualBlockIoProtocol::new(vbio).install(self.bt)?;
        let virtual_handle = registered.handle();
        let device_path = registered.device_path().to_vec();

        let simple_file_system = if self.iso.image_format.is_iso() {
            match self.install_iso_simple_file_system(
                &source_block_io,
                virtual_handle,
                efi_file_replacements,
            ) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install SimpleFileSystem on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        } else {
            None
        };

        let load_file_protocol = if load_file_entries.is_empty() {
            None
        } else {
            match PreloadedLoadFileProtocol::install(self.bt, virtual_handle, load_file_entries) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install LoadFile protocols on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        };

        registered.leak();
        if let Some(protocol) = simple_file_system {
            protocol.leak();
        }
        if let Some(protocol) = load_file_protocol {
            protocol.leak();
        }

        info!(
            "Virtual Block IO installed on {:?}: {:?}, source extents: {}",
            virtual_handle,
            virtual_info,
            self.iso.extents.len()
        );

        Ok(VirtualBootDevice {
            handle: virtual_handle,
            device_path,
        })
    }

    fn build_conf_replace_overlays(
        &self,
        source_block_io: &BlockIO,
        config: &mut VirtualDeviceConfig,
    ) -> uefi::Result<Vec<MemoryOverlay>> {
        let Some(plugin) = self.iso.ventoy_plugin.as_ref() else {
            return Ok(Vec::new());
        };
        if plugin.conf_replace.is_empty() {
            return Ok(Vec::new());
        }
        if !self.iso.image_format.is_iso() {
            return Ok(Vec::new());
        }
        let source_fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        let mut overlays = Vec::new();
        overlays
            .try_reserve_exact(plugin.conf_replace.len().saturating_mul(3))
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        let mut next_append_offset =
            align_up_u64(config.iso_size, ISO9660_SECTOR_SIZE).ok_or(Status::OUT_OF_RESOURCES)?;

        if self.iso.is_udf {
            let udf_fs = self.open_udf_filesystem(source_block_io)?;
            for rule in &plugin.conf_replace {
                let replacement = match source_fs.load_file(&rule.new_path) {
                    Ok(file) => file,
                    Err(err) => {
                        warn!(
                            "Ventoy UDF conf_replace new path {} for {} was not loaded: {:?}",
                            rule.new_path,
                            self.iso.path,
                            err.status()
                        );
                        continue;
                    }
                };
                if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                    warn!(
                        "Ventoy UDF conf_replace new path {} for {} is too large: {} bytes",
                        replacement.path,
                        self.iso.path,
                        replacement.data.len()
                    );
                    continue;
                }

                let aligned_len = align_up(replacement.data.len(), ISO9660_SECTOR_SIZE as usize)
                    .ok_or(Status::OUT_OF_RESOURCES)?;
                let replacement_size =
                    u64::try_from(replacement.data.len()).map_err(|_| Status::OUT_OF_RESOURCES)?;
                let replacement_sector = next_append_offset / ISO9660_SECTOR_SIZE;

                let patch = match udf_fs.file_replacement_patch(
                    &rule.org,
                    replacement_sector,
                    replacement_size,
                    aligned_len as u64,
                ) {
                    Ok(patch) => patch,
                    Err(err) => {
                        warn!(
                            "Ventoy UDF conf_replace org path {} for {} was not patched: {:?}",
                            rule.org, self.iso.path, err
                        );
                        continue;
                    }
                };

                overlays.push(MemoryOverlay::new(
                    patch.file_entry_offset,
                    patch.file_entry_data,
                ));
                if let Some(partition_descriptor) = patch.partition_descriptor {
                    overlays.push(MemoryOverlay::new(
                        partition_descriptor.descriptor_offset,
                        partition_descriptor.descriptor_data,
                    ));
                }

                let mut data = replacement.data;
                data.resize(aligned_len, 0);
                overlays.push(MemoryOverlay::new(next_append_offset, data));

                info!(
                    "Prepared Ventoy UDF conf_replace for {}: {} -> {} at virtual sector {} ({} bytes)",
                    self.iso.path, rule.org, replacement.path, replacement_sector, replacement_size
                );
                next_append_offset = next_append_offset
                    .checked_add(aligned_len as u64)
                    .ok_or(Status::OUT_OF_RESOURCES)?;
            }

            if !overlays.is_empty() {
                config.iso_size = config.iso_size.max(next_append_offset);
                info!(
                    "Prepared {} Ventoy UDF conf_replace overlay(s) for {}; virtual size now {} bytes",
                    overlays.len(),
                    self.iso.path,
                    config.iso_size
                );
            }

            return Ok(overlays);
        }

        let iso_fs = self.open_iso9660_filesystem(source_block_io)?;
        for rule in &plugin.conf_replace {
            let record = match iso_fs.directory_record_location(&rule.org) {
                Ok(record) if !record.is_dir => record,
                Ok(_) => {
                    warn!(
                        "Ventoy conf_replace org path {} for {} is a directory",
                        rule.org, self.iso.path
                    );
                    continue;
                }
                Err(err) => {
                    warn!(
                        "Ventoy conf_replace org path {} for {} was not found: {:?}",
                        rule.org, self.iso.path, err
                    );
                    continue;
                }
            };

            let replacement = match source_fs.load_file(&rule.new_path) {
                Ok(file) => file,
                Err(err) => {
                    warn!(
                        "Ventoy conf_replace new path {} for {} was not loaded: {:?}",
                        rule.new_path,
                        self.iso.path,
                        err.status()
                    );
                    continue;
                }
            };
            if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                warn!(
                    "Ventoy conf_replace new path {} for {} is too large: {} bytes",
                    replacement.path,
                    self.iso.path,
                    replacement.data.len()
                );
                continue;
            }

            let aligned_len = align_up(replacement.data.len(), ISO9660_SECTOR_SIZE as usize)
                .ok_or(Status::OUT_OF_RESOURCES)?;
            let replacement_size =
                u32::try_from(replacement.data.len()).map_err(|_| Status::OUT_OF_RESOURCES)?;
            let replacement_sector = u32::try_from(next_append_offset / ISO9660_SECTOR_SIZE)
                .map_err(|_| Status::OUT_OF_RESOURCES)?;
            let patch_offset = record
                .record_offset
                .checked_add(2)
                .ok_or(Status::OUT_OF_RESOURCES)?;

            overlays.push(MemoryOverlay::new(
                patch_offset,
                iso9660_file_extent_patch(replacement_sector, replacement_size),
            ));

            let mut data = replacement.data;
            data.resize(aligned_len, 0);
            overlays.push(MemoryOverlay::new(next_append_offset, data));

            info!(
                "Prepared Ventoy conf_replace for {}: {} -> {} at virtual sector {} ({} bytes)",
                self.iso.path, rule.org, replacement.path, replacement_sector, replacement_size
            );
            next_append_offset = next_append_offset
                .checked_add(aligned_len as u64)
                .ok_or(Status::OUT_OF_RESOURCES)?;
        }

        if !overlays.is_empty() {
            config.iso_size = config.iso_size.max(next_append_offset);
            info!(
                "Prepared {} Ventoy conf_replace overlay(s) for {}; virtual size now {} bytes",
                overlays.len() / 2,
                self.iso.path,
                config.iso_size
            );
        }

        Ok(overlays)
    }

    fn build_efi_file_replacements(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<Vec<VirtualFileReplacement>> {
        let Some(plugin) = self.iso.ventoy_plugin.as_ref() else {
            return Ok(Vec::new());
        };
        if plugin.conf_replace.is_empty() || !self.iso.image_format.is_iso() {
            return Ok(Vec::new());
        }

        let img_replace_count = plugin
            .conf_replace
            .iter()
            .filter(|rule| rule.img.unwrap_or(0) > 0)
            .count();
        if img_replace_count == 0 {
            return Ok(Vec::new());
        }

        let source_fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        let mut replacements = Vec::new();
        replacements
            .try_reserve_exact(img_replace_count)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        for rule in plugin
            .conf_replace
            .iter()
            .filter(|rule| rule.img.unwrap_or(0) > 0)
        {
            let replacement = match source_fs.load_file(&rule.new_path) {
                Ok(file) => file,
                Err(err) => {
                    warn!(
                        "Ventoy EFI img_replace new path {} for {} was not loaded: {:?}",
                        rule.new_path,
                        self.iso.path,
                        err.status()
                    );
                    continue;
                }
            };
            if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                warn!(
                    "Ventoy EFI img_replace new path {} for {} is too large: {} bytes",
                    replacement.path,
                    self.iso.path,
                    replacement.data.len()
                );
                continue;
            }

            info!(
                "Prepared Ventoy EFI img_replace for {}: {} -> {} ({} bytes)",
                self.iso.path,
                rule.org,
                replacement.path,
                replacement.data.len()
            );
            replacements.push(VirtualFileReplacement::new(&rule.org, replacement.data));
        }

        Ok(replacements)
    }
}
