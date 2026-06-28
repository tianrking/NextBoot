use super::candidates::{ventoy_arch_cpio_candidates, VENTOY_COMMON_CPIO_CANDIDATES};
use super::errors::ventoy_linux_error_to_uefi_status;
use super::source_volume::SourceVolumeFile;
use super::util::{selected_persistence_backend_index, selected_ventoy_plugin_index};
use super::BootManager;
use crate::scanner::IsoExtent;
use crate::ventoy_linux::{VentoyDudFile, VentoyLinuxInitrdInput};
use alloc::vec::Vec;
use log::{info, warn};
use uefi::Status;

impl BootManager<'_> {
    pub(super) fn append_ventoy_linux_initrd_overlay(
        &self,
        initrd_data: &mut Vec<u8>,
    ) -> uefi::Result<()> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        let boot_config = self.boot_virtual_config();
        let (os_param, _, _) = self.build_ventoy_os_param_payload(&boot_config)?;
        let image_map = self.build_ventoy_linux_image_map()?;

        let mut base_archives = Vec::new();
        self.try_load_ventoy_cpio_archives(&mut base_archives)?;
        let base_refs: Vec<&[u8]> = base_archives
            .iter()
            .map(|file: &SourceVolumeFile| file.data.as_slice())
            .collect();

        let plugin = self.iso.ventoy_plugin.as_ref();
        let auto_install = self.load_selected_auto_install_template(plugin)?;
        let persistent_map = self.load_selected_persistence_map(plugin)?;
        let injection = self.load_plugin_injection_archive(plugin)?;
        let dud_files = self.load_plugin_dud_files(plugin)?;
        let dud_refs: Vec<VentoyDudFile<'_>> = dud_files
            .iter()
            .map(|file| VentoyDudFile {
                source_path: file.path.as_str(),
                data: file.data.as_slice(),
            })
            .collect();

        let input = VentoyLinuxInitrdInput {
            base_archives: &base_refs,
            original_initrd: Some(initrd_data.as_slice()),
            image_map: &image_map,
            os_param: &os_param,
            auto_install: auto_install.as_ref().map(|file| file.data.as_slice()),
            persistent_map: persistent_map.as_deref(),
            injection_archive: injection.as_ref().map(|file| file.data.as_slice()),
            dud_files: &dud_refs,
        };
        let replacement = crate::ventoy_linux::build_ventoy_linux_initrd(&input)
            .map_err(ventoy_linux_error_to_uefi_status)?;
        let original_initrd_len = initrd_data.len();

        initrd_data.clear();
        initrd_data
            .try_reserve_exact(replacement.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        initrd_data.extend_from_slice(&replacement);

        info!(
            "Prepared Ventoy Linux initrd: {} bytes, original_initrd={} bytes, base_archives={}, image_chunks={}, auto_install={}, persistence={}, injection={}, dud_files={}",
            initrd_data.len(),
            original_initrd_len,
            base_archives.len(),
            image_map.len(),
            auto_install.is_some(),
            persistent_map.as_ref().map_or(0, Vec::len),
            injection.is_some(),
            dud_refs.len()
        );

        Ok(())
    }

    fn build_ventoy_linux_image_map(
        &self,
    ) -> uefi::Result<Vec<crate::ventoy_linux::VentoyImageMapChunk>> {
        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(self.iso.block_size, |disk| disk.block_size);
        if disk_sector_size != self.iso.block_size {
            warn!(
                "Ventoy Linux initrd map source disk sector size {} differs from volume sector size {} for {}",
                disk_sector_size, self.iso.block_size, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let extents = self.ventoy_source_extents()?;
        crate::ventoy_linux::build_image_map_chunks(&extents, self.iso.block_size, 2048)
            .map_err(ventoy_linux_error_to_uefi_status)
            .map_err(Into::into)
    }

    fn try_load_ventoy_cpio_archives(
        &self,
        archives: &mut Vec<SourceVolumeFile>,
    ) -> uefi::Result<()> {
        for path in VENTOY_COMMON_CPIO_CANDIDATES
            .iter()
            .chain(ventoy_arch_cpio_candidates().iter())
        {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded Ventoy cpio archive {} ({} bytes)",
                        path,
                        file.data.len()
                    );
                    archives
                        .try_reserve_exact(1)
                        .map_err(|_| Status::OUT_OF_RESOURCES)?;
                    archives.push(file);
                }
                Err(err) => {
                    info!(
                        "Ventoy cpio archive {} not loaded: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Ok(())
    }

    pub(super) fn load_selected_auto_install_template(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let Some(auto_install) = plugin.and_then(|plugin| plugin.auto_install.as_ref()) else {
            return Ok(None);
        };
        let Some(index) =
            selected_ventoy_plugin_index(auto_install.autosel, auto_install.templates.len())
        else {
            return Ok(None);
        };
        let Some(path) = auto_install.templates.get(index) else {
            return Ok(None);
        };

        match self.load_source_volume_file(path) {
            Ok(file) => Ok(Some(file)),
            Err(err) => {
                warn!(
                    "Ventoy auto_install template {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                Ok(None)
            }
        }
    }

    fn load_selected_persistence_map(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<Vec<crate::ventoy_linux::VentoyImageMapChunk>>> {
        let Some(persistence) = plugin.and_then(|plugin| plugin.persistence.as_ref()) else {
            return Ok(None);
        };
        let Some(index) =
            selected_persistence_backend_index(persistence.autosel, persistence.backends.len())
        else {
            info!(
                "Ventoy persistence is configured for {}, but no backend is selected",
                self.iso.path
            );
            return Ok(None);
        };
        let Some(path) = persistence.backends.get(index) else {
            return Ok(None);
        };

        let metadata = match self.source_volume_file_metadata(path) {
            Ok(metadata) => metadata,
            Err(err) => {
                warn!(
                    "Ventoy persistence backend {} for {} was not mapped: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                return Ok(None);
            }
        };

        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(metadata.block_size, |disk| disk.block_size);
        if disk_sector_size != metadata.block_size {
            warn!(
                "Ventoy persistence backend {} source disk sector size {} differs from volume sector size {}",
                metadata.path, disk_sector_size, metadata.block_size
            );
            return Ok(None);
        }

        let extents = self.ventoy_source_volume_extents(&metadata.extents)?;
        let chunks =
            match crate::ventoy_linux::build_image_map_chunks(&extents, metadata.block_size, 512) {
                Ok(chunks) if !chunks.is_empty() => chunks,
                Ok(_) => {
                    warn!(
                        "Ventoy persistence backend {} for {} has no mapped extents",
                        metadata.path, self.iso.path
                    );
                    return Ok(None);
                }
                Err(err) => {
                    warn!(
                        "Ventoy persistence backend {} for {} has unsupported extents: {:?}",
                        metadata.path, self.iso.path, err
                    );
                    return Ok(None);
                }
            };

        info!(
            "Mapped Ventoy persistence backend {} for {}: {} chunks, block_size={}",
            metadata.path,
            self.iso.path,
            chunks.len(),
            metadata.block_size
        );
        Ok(Some(chunks))
    }

    fn ventoy_source_volume_extents(
        &self,
        source_extents: &[IsoExtent],
    ) -> uefi::Result<Vec<crate::ventoy::VentoyExtent>> {
        let mut extents = Vec::new();
        extents
            .try_reserve_exact(source_extents.len())
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        let disk_lba_offset = self
            .iso
            .source_disk
            .map_or(0, |disk| disk.partition_start_lba);
        for extent in source_extents {
            extents.push(crate::ventoy::VentoyExtent {
                virtual_block_start: extent.virtual_block_start,
                physical_lba: extent
                    .physical_lba
                    .checked_add(disk_lba_offset)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                block_count: extent.block_count,
            });
        }

        Ok(extents)
    }

    pub(super) fn load_plugin_injection_archive(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let Some(path) = plugin.and_then(|plugin| plugin.injection_archive.as_deref()) else {
            return Ok(None);
        };

        match self.load_source_volume_file(path) {
            Ok(file) => Ok(Some(file)),
            Err(err) => {
                warn!(
                    "Ventoy injection archive {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                Ok(None)
            }
        }
    }

    fn load_plugin_dud_files(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Vec<SourceVolumeFile>> {
        let mut files = Vec::new();
        let Some(dud) = plugin.and_then(|plugin| plugin.dud.as_ref()) else {
            return Ok(files);
        };

        files
            .try_reserve_exact(dud.files.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        for path in &dud.files {
            match self.load_source_volume_file(path) {
                Ok(file) => files.push(file),
                Err(err) => warn!(
                    "Ventoy DUD file {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                ),
            }
        }

        Ok(files)
    }
}
