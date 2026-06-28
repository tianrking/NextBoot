use super::candidates::*;
use super::errors::{
    ventoy_windows_runtime_data_error_to_uefi_status,
    ventoy_windows_wimboot_payload_error_to_uefi_status,
};
use super::source_volume::{SourceVolumeFile, SourceVolumeReader};
use super::wimboot_callbacks::WimbootRuntimeInputs;
use super::wimboot_runtime::WimbootRuntimeFile;
use super::{BootManager, VirtualBootDevice};
use crate::wimboot::{self, WimbootVirtualFile};
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_virtio::VirtualDeviceConfig;
use uefi::proto::media::block::BlockIO;
use uefi::Status;

impl BootManager<'_> {
    pub(super) fn prepare_wimboot(&self) -> uefi::Result<()> {
        let Some(wim_info) = self.iso.wim_info else {
            warn!("WIM/ESD file has no parsed WIM metadata: {}", self.iso.path);
            return Err(Status::LOAD_ERROR.into());
        };

        if !wim_info.wimboot_supported {
            warn!(
                "WIMBOOT does not support {}: boot_index={} in_range={} compression={:?}",
                self.iso.path,
                wim_info.boot_index,
                wim_info.boot_index_in_range,
                wim_info.compression
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let helper = self.load_wimboot_helper()?;
        let inputs = self.prepare_wimboot_runtime_inputs(&helper)?;
        let runtime = self.register_wimboot_runtime_files(inputs.runtime_files)?;
        let callbacks = runtime.callbacks();
        let load_options = wimboot::build_wimboot_command_line(
            &inputs.virtual_files,
            Some(callbacks),
            Some(wim_info.boot_index),
        )
        .map_err(|_| Status::INVALID_PARAMETER)?;

        info!(
            "Prepared WIMBOOT load options for {}: {}",
            self.iso.path, load_options
        );
        info!(
            "Registered WIMBOOT runtime file callbacks: pfsize=0x{:x} pfread=0x{:x}",
            callbacks.file_size, callbacks.file_read
        );

        let source_device = VirtualBootDevice {
            handle: self.iso.volume_handle,
            device_path: self.handle_device_path_bytes(self.iso.volume_handle)?,
        };

        self.chain_load_with_options(
            &source_device,
            &helper.path,
            &helper.data,
            Some(&load_options),
        )
    }

    fn prepare_wimboot_runtime_inputs(
        &self,
        helper: &SourceVolumeFile,
    ) -> uefi::Result<WimbootRuntimeInputs> {
        let boot_wim = WimbootRuntimeFile::from_iso(self.iso, WIMBOOT_BOOT_WIM_CALLBACK_PATH)?;
        let mut internal = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
                .ok_or(uefi::Status::DEVICE_ERROR)?;
            let boot_index = self.iso.wim_info.map(|info| info.boot_index).unwrap_or(0);
            self.collect_wimboot_internal_files(&reader, &boot_wim, boot_index)
        };

        let mut runtime_files = Vec::new();
        runtime_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        runtime_files.push(boot_wim);
        runtime_files.push(WimbootRuntimeFile::from_memory(
            WIMBOOT_SELF_CALLBACK_PATH,
            helper.data.clone(),
        ));
        let mut include_bootmgfw = false;
        if let Some(bootmgfw) = internal.bootmgfw.take() {
            runtime_files.push(bootmgfw);
            include_bootmgfw = true;
        }

        let mut include_bcd = false;
        match self
            .find_source_volume_file(WIMBOOT_BCD_CANDIDATES, WIMBOOT_COMPRESSED_BCD_CANDIDATES)
        {
            Ok(mut bcd) => {
                let patched = wimboot::patch_bcd_for_efi(&mut bcd.data);
                if patched != 0 {
                    info!(
                        "Patched {} UTF-16 BCD .exe reference(s) for UEFI WIMBOOT",
                        patched
                    );
                }
                runtime_files.push(WimbootRuntimeFile::from_memory(
                    WIMBOOT_BCD_CALLBACK_PATH,
                    bcd.data,
                ));
                include_bcd = true;
            }
            Err(err) if err.status() == Status::NOT_FOUND => {
                if let Some(bcd) = internal.bcd.take() {
                    runtime_files.push(bcd);
                    include_bcd = true;
                } else {
                    info!("WIMBOOT BCD was not found externally; relying on boot.wim extraction");
                }
            }
            Err(err) => return Err(err),
        }

        let include_boot_sdi =
            match self.find_optional_source_volume_file_metadata(WIMBOOT_BOOT_SDI_CANDIDATES) {
                Ok(Some(file)) => {
                    runtime_files.push(WimbootRuntimeFile::from_source_file(
                        &file,
                        WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                    )?);
                    true
                }
                Ok(None) => {
                    if let Some(boot_sdi) = internal.boot_sdi.take() {
                        runtime_files.push(boot_sdi);
                        true
                    } else {
                        info!("WIMBOOT boot.sdi was not found; relying on boot.wim extraction");
                        false
                    }
                }
                Err(err) => return Err(err),
            };

        let mut virtual_files = Vec::new();
        virtual_files
            .try_reserve_exact(5)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        virtual_files.push(
            WimbootVirtualFile::new("boot.wim", WIMBOOT_BOOT_WIM_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        virtual_files.push(
            WimbootVirtualFile::new("vtoy_wimboot", WIMBOOT_SELF_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        if include_bootmgfw {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME,
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_bcd {
            virtual_files.push(
                WimbootVirtualFile::new("bcd", WIMBOOT_BCD_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_boot_sdi {
            virtual_files.push(
                WimbootVirtualFile::new("boot.sdi", WIMBOOT_BOOT_SDI_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }

        Ok(WimbootRuntimeInputs {
            runtime_files,
            virtual_files,
        })
    }

    pub(super) fn prepare_windows_iso_wimboot(&self) -> uefi::Result<()> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        if self.iso.ventoy_windows11_bypass_check || self.iso.ventoy_windows11_bypass_nro {
            info!(
                "Windows 11 bypass controls requested for {}: hardware_check={} nro={}",
                self.iso.path,
                self.iso.ventoy_windows11_bypass_check,
                self.iso.ventoy_windows11_bypass_nro
            );
        }

        let helper = self.load_wimboot_helper()?;
        let boot_config = self.boot_virtual_config();
        let inputs = self.prepare_windows_iso_wimboot_runtime_inputs(&helper, &boot_config)?;
        let runtime = self.register_wimboot_runtime_files(inputs.runtime_files)?;
        let callbacks = runtime.callbacks();
        let load_options =
            wimboot::build_wimboot_command_line(&inputs.virtual_files, Some(callbacks), None)
                .map_err(|_| Status::INVALID_PARAMETER)?;

        info!(
            "Prepared Windows ISO WIMBOOT fallback for {}: {}",
            self.iso.path, load_options
        );

        let source_device = VirtualBootDevice {
            handle: self.iso.volume_handle,
            device_path: self.handle_device_path_bytes(self.iso.volume_handle)?,
        };

        self.chain_load_with_options(
            &source_device,
            &helper.path,
            &helper.data,
            Some(&load_options),
        )
    }

    fn prepare_windows_iso_wimboot_runtime_inputs(
        &self,
        helper: &SourceVolumeFile,
        boot_config: &VirtualDeviceConfig,
    ) -> uefi::Result<WimbootRuntimeInputs> {
        let (boot_wim, bcd, boot_sdi) = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let fs = self.open_virtual_iso_filesystem(&source_block_io)?;

            let boot_wim = self.find_iso_file_metadata(&fs, WINDOWS_ISO_BOOT_WIM_CANDIDATES)?;
            let bcd = self.find_optional_iso_file_data(&fs, WINDOWS_ISO_BCD_CANDIDATES)?;
            let boot_sdi =
                self.find_optional_iso_file_data(&fs, WINDOWS_ISO_BOOT_SDI_CANDIDATES)?;
            (boot_wim, bcd, boot_sdi)
        };
        let boot_wim_runtime = WimbootRuntimeFile::from_mapped_segments(
            WIMBOOT_BOOT_WIM_CALLBACK_PATH,
            boot_wim.size,
            boot_wim.segments,
        )?;
        let mut internal = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
                .ok_or(uefi::Status::DEVICE_ERROR)?;
            self.collect_wimboot_internal_files(&reader, &boot_wim_runtime, 0)
        };

        let mut runtime_files = Vec::new();
        runtime_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        runtime_files.push(boot_wim_runtime);
        runtime_files.push(WimbootRuntimeFile::from_memory(
            WIMBOOT_SELF_CALLBACK_PATH,
            helper.data.clone(),
        ));

        let mut include_bootmgfw = false;
        if let Some(bootmgfw) = internal.bootmgfw.take() {
            runtime_files.push(bootmgfw);
            include_bootmgfw = true;
        }

        let mut include_winpeshl = false;
        if let Some(original_winpeshl) = internal.winpeshl.take() {
            match self.prepare_windows_wimboot_jump_payload(boot_config, &original_winpeshl) {
                Ok(Some(data)) => {
                    info!(
                        "Prepared Ventoy Windows WIMBOOT jump payload for {}: {} bytes",
                        self.iso.path,
                        data.len()
                    );
                    runtime_files.push(WimbootRuntimeFile::from_memory(
                        WIMBOOT_WINPESHL_CALLBACK_PATH,
                        data,
                    ));
                    include_winpeshl = true;
                }
                Ok(None) => {}
                Err(err) => warn!(
                    "Ventoy Windows WIMBOOT jump payload for {} was not prepared: {:?}",
                    self.iso.path,
                    err.status()
                ),
            }
        }

        let mut include_bcd = false;
        if let Some(mut bcd) = bcd {
            let patched = wimboot::patch_bcd_for_efi(&mut bcd.data);
            if patched != 0 {
                info!(
                    "Patched {} UTF-16 Windows ISO BCD .exe reference(s) for UEFI WIMBOOT",
                    patched
                );
            }
            runtime_files.push(WimbootRuntimeFile::from_memory(
                WIMBOOT_BCD_CALLBACK_PATH,
                bcd.data,
            ));
            include_bcd = true;
        } else if let Some(bcd) = internal.bcd.take() {
            runtime_files.push(bcd);
            include_bcd = true;
        } else {
            info!("Windows ISO BCD was not found externally; relying on boot.wim extraction");
        }

        let mut include_boot_sdi = false;
        if let Some(boot_sdi) = boot_sdi {
            runtime_files.push(WimbootRuntimeFile::from_memory(
                WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                boot_sdi.data,
            ));
            include_boot_sdi = true;
        } else if let Some(boot_sdi) = internal.boot_sdi.take() {
            runtime_files.push(boot_sdi);
            include_boot_sdi = true;
        } else {
            info!("Windows ISO boot.sdi was not found externally; relying on boot.wim extraction");
        }

        let mut virtual_files = Vec::new();
        virtual_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        virtual_files.push(
            WimbootVirtualFile::new("boot.wim", WIMBOOT_BOOT_WIM_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        virtual_files.push(
            WimbootVirtualFile::new("vtoy_wimboot", WIMBOOT_SELF_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        if include_bootmgfw {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME,
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_winpeshl {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_WINPESHL_VIRTUAL_NAME,
                    WIMBOOT_WINPESHL_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_bcd {
            virtual_files.push(
                WimbootVirtualFile::new("bcd", WIMBOOT_BCD_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_boot_sdi {
            virtual_files.push(
                WimbootVirtualFile::new("boot.sdi", WIMBOOT_BOOT_SDI_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }

        Ok(WimbootRuntimeInputs {
            runtime_files,
            virtual_files,
        })
    }

    fn prepare_windows_wimboot_jump_payload(
        &self,
        boot_config: &VirtualDeviceConfig,
        original_winpeshl: &[u8],
    ) -> uefi::Result<Option<Vec<u8>>> {
        let jump = match self.find_source_volume_file(VTOYJUMP_CANDIDATES, &[]) {
            Ok(file) => file,
            Err(err) if err.status() == Status::NOT_FOUND => {
                info!(
                    "Ventoy Windows jump helper was not found for {}; skipping winpeshl.exe overlay",
                    self.iso.path
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

        let plugin = self.iso.ventoy_plugin.as_ref();
        let auto_install = self.load_selected_auto_install_template(plugin)?;
        let injection = self.load_plugin_injection_archive(plugin)?;
        let runtime_input = nextboot_windows::VentoyWindowsRuntimeDataInput {
            auto_install: auto_install.as_ref().map(|file| {
                nextboot_windows::VentoyWindowsAutoInstall {
                    source_path: file.path.as_str(),
                    data: file.data.as_slice(),
                }
            }),
            injection_archive: injection.as_ref().map(|file| file.path.as_str()),
            windows11_bypass_check: self.iso.ventoy_windows11_bypass_check,
            windows11_bypass_nro: self.iso.ventoy_windows11_bypass_nro,
        };
        let windows_data = nextboot_windows::build_ventoy_windows_runtime_data(runtime_input)
            .map_err(ventoy_windows_runtime_data_error_to_uefi_status)?;
        let (os_param, image_chunks, image_location_addr) =
            self.build_ventoy_os_param_payload(boot_config)?;
        let payload = nextboot_windows::build_ventoy_wimboot_jump_payload(
            jump.data.as_slice(),
            &os_param,
            windows_data.as_slice(),
            original_winpeshl,
        )
        .map_err(ventoy_windows_wimboot_payload_error_to_uefi_status)?;

        info!(
            "Built Ventoy Windows runtime data for {}: jump={}, auto_install={}, injection={}, win11_bypass_check={}, win11_bypass_nro={}, image_chunks={}, image_location=0x{:x}",
            self.iso.path,
            jump.path,
            auto_install.is_some(),
            injection.is_some(),
            self.iso.ventoy_windows11_bypass_check,
            self.iso.ventoy_windows11_bypass_nro,
            image_chunks,
            image_location_addr
        );

        Ok(Some(payload))
    }
}
