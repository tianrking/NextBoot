use super::candidates::*;
use super::errors::wim_read_error_to_uefi_status;
use super::source_volume::{SourceVolumeFile, SourceVolumeReader};
use super::wimboot_callbacks::WimbootRuntimeRegistration;
use super::wimboot_runtime::{
    WimbootInternalFiles, WimbootRuntimeContext, WimbootRuntimeFile, WimbootWimImage,
};
use super::BootManager;
use crate::{wim, wimboot};
use alloc::vec::Vec;
use log::{info, warn};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::BootServices;
use uefi::Status;

impl<'a> BootManager<'a> {
    pub(super) fn collect_wimboot_internal_files(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        boot_index: u32,
    ) -> WimbootInternalFiles {
        let image = match self.load_wimboot_wim_image(reader, boot_wim, boot_index) {
            Ok(image) => image,
            Err(err) => {
                warn!(
                    "Could not inspect WIM internals for {}: {:?}",
                    self.iso.path,
                    err.status()
                );
                return WimbootInternalFiles::default();
            }
        };

        let bootmgfw = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BOOTMGFW_CANDIDATES)
            .map(|resource| {
                info!(
                    "Registered WIM internal {} ({} bytes)",
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME, resource.uncompressed_size
                );
                WimbootRuntimeFile::from_wim_resource(
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                    boot_wim,
                    image.metadata,
                    resource,
                )
            });

        let boot_sdi = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BOOT_SDI_CANDIDATES)
            .map(|resource| {
                info!(
                    "Registered WIM internal boot.sdi ({} bytes)",
                    resource.uncompressed_size
                );
                WimbootRuntimeFile::from_wim_resource(
                    WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                    boot_wim,
                    image.metadata,
                    resource,
                )
            });

        let bcd = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BCD_CANDIDATES)
            .and_then(|resource| {
                match self.read_wim_resource_to_vec(reader, boot_wim, &image.metadata, &resource) {
                    Ok(mut data) => {
                        let patched = wimboot::patch_bcd_for_efi(&mut data);
                        if patched != 0 {
                            info!(
                                "Patched {} UTF-16 WIM internal BCD .exe reference(s) for UEFI WIMBOOT",
                                patched
                            );
                        }
                        info!("Registered WIM internal BCD ({} bytes)", data.len());
                        Some(WimbootRuntimeFile::from_memory(WIMBOOT_BCD_CALLBACK_PATH, data))
                    }
                    Err(err) => {
                        warn!(
                            "Could not read WIM internal BCD for {}: {:?}",
                            self.iso.path,
                            err.status()
                        );
                        None
                    }
                }
            });

        let winpeshl = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_WINPESHL_CANDIDATES)
            .and_then(|resource| {
                match self.read_wim_resource_to_vec(reader, boot_wim, &image.metadata, &resource) {
                    Ok(data) => {
                        info!("Loaded WIM internal winpeshl.exe ({} bytes)", data.len());
                        Some(data)
                    }
                    Err(err) => {
                        warn!(
                            "Could not read WIM internal winpeshl.exe for {}: {:?}",
                            self.iso.path,
                            err.status()
                        );
                        None
                    }
                }
            });

        WimbootInternalFiles {
            bootmgfw,
            bcd,
            boot_sdi,
            winpeshl,
        }
    }

    fn load_wimboot_wim_image(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        boot_index: u32,
    ) -> uefi::Result<WimbootWimImage> {
        let mut header = [0u8; wim::WIM_HEADER_SIZE];
        boot_wim
            .read_range(reader, 0, &mut header)
            .ok_or(Status::DEVICE_ERROR)?;
        let metadata = wim::parse_wim_metadata(&header).ok_or(Status::LOAD_ERROR)?;
        if !metadata.is_wimboot_supported() {
            return Err(Status::UNSUPPORTED.into());
        }

        let lookup =
            self.read_wim_resource_to_vec(reader, boot_wim, &metadata, &metadata.lookup)?;
        let image_index = boot_index;
        let image_metadata_resource =
            wim::metadata_resource_for_image(&metadata, &lookup, image_index)
                .ok_or(Status::NOT_FOUND)?;
        let image_metadata =
            self.read_wim_resource_to_vec(reader, boot_wim, &metadata, &image_metadata_resource)?;

        Ok(WimbootWimImage {
            metadata,
            lookup,
            image_metadata,
        })
    }

    fn read_wim_resource_to_vec(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        metadata: &wim::WimMetadata,
        resource: &wim::WimResourceHeader,
    ) -> uefi::Result<Vec<u8>> {
        let len =
            usize::try_from(resource.uncompressed_size).map_err(|_| Status::OUT_OF_RESOURCES)?;
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        out.resize(len, 0);
        wim::read_resource_range_with(
            metadata,
            boot_wim.size,
            resource,
            0,
            &mut out,
            |offset, buf| {
                boot_wim
                    .read_range(reader, offset, buf)
                    .ok_or(wim::WimReadError::ResourceOutOfBounds)
            },
        )
        .map_err(wim_read_error_to_uefi_status)?;
        Ok(out)
    }

    fn find_wim_resource_file(
        &self,
        image: &WimbootWimImage,
        candidates: &[&str],
    ) -> Option<wim::WimResourceHeader> {
        for path in candidates {
            match wim::file_resource_for_path(&image.image_metadata, &image.lookup, path) {
                Ok(resource) => return Some(resource),
                Err(wim::WimPathError::NotFound | wim::WimPathError::ResourceNotFound) => {}
                Err(err) => {
                    warn!(
                        "WIM internal file candidate {} failed for {}: {:?}",
                        path, self.iso.path, err
                    );
                }
            }
        }

        None
    }

    pub(super) fn register_wimboot_runtime_files(
        &self,
        files: Vec<WimbootRuntimeFile>,
    ) -> uefi::Result<WimbootRuntimeRegistration<'a>> {
        let bt: &'a BootServices = self.bt;
        let source_block_io = bt.open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;

        Ok(WimbootRuntimeRegistration::install(
            WimbootRuntimeContext { reader, files },
            source_block_io,
        ))
    }

    pub(super) fn load_wimboot_helper(&self) -> uefi::Result<SourceVolumeFile> {
        let candidates = wimboot_helper_candidates();
        if candidates.is_empty() {
            warn!("WIMBOOT EFI helper is not available for this firmware architecture");
            return Err(Status::UNSUPPORTED.into());
        }

        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded WIMBOOT helper {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "WIMBOOT helper candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        for path in compressed_wimboot_helper_candidates() {
            match self.load_compressed_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded compressed WIMBOOT helper {} -> {} bytes",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Compressed WIMBOOT helper candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }
}
