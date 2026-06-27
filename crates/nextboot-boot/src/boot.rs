//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::scanner::{ImageFormat, IsoFile, OsType};
use crate::vdi;
use crate::vhdx;
use crate::wim;
use crate::wimboot::{self, WimbootVirtualFile};
use alloc::string::String;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_fs::FileExtent;
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::{MemoryOverlay, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::BootServices;
use uefi::table::runtime::RuntimeServices;
use uefi::{Handle, Status};

mod candidates;
mod chain_load;
mod errors;
mod file_access;
mod linux;
mod linux_ventoy;
mod load_file;
mod os_param;
mod source_volume;
mod util;
mod vhd;
mod virtual_boot;
mod virtual_device;
mod wimboot_runtime;
use candidates::*;
use errors::{
    ventoy_windows_runtime_data_error_to_uefi_status,
    ventoy_windows_wimboot_payload_error_to_uefi_status, virtio_error_to_uefi_status,
    wim_read_error_to_uefi_status,
};
use source_volume::{SourceVolumeFile, SourceVolumeReader, ZeroPhysicalReader};
use util::*;
use wimboot_runtime::{
    WimbootInternalFiles, WimbootMappedSegment, WimbootRuntimeContext, WimbootRuntimeFile,
    WimbootRuntimeInputs, WimbootRuntimeRegistration, WimbootWimImage,
};

/// 引导管理器
pub struct BootManager<'a> {
    bt: &'a BootServices,
    rt: &'a RuntimeServices,
    parent_image: Handle,
    iso: &'a IsoFile,
}

impl<'a> BootManager<'a> {
    /// 创建新的引导管理器
    pub fn new(
        bt: &'a BootServices,
        rt: &'a RuntimeServices,
        parent_image: Handle,
        iso: &'a IsoFile,
    ) -> Self {
        Self {
            bt,
            rt,
            parent_image,
            iso,
        }
    }

    /// 准备并执行引导
    pub fn prepare_and_boot(&self) -> uefi::Result<()> {
        info!("Preparing to boot: {}", self.iso.path);
        if self.iso.image_format.is_efi_executable() {
            return self.boot_efi_executable();
        }
        if self.iso.image_format.is_wim_container() {
            return self.prepare_wimboot();
        }

        if !self.iso.image_format.supports_virtual_disk_boot() {
            warn!(
                "Image format {} is recognized but not bootable yet: {}",
                self.iso.image_format, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let boot_config = self.boot_virtual_config();
        if let Err(err) = self.publish_os_param(&boot_config) {
            warn!(
                "Failed to publish {} for {}: {:?}",
                os_param::NEXTBOOT_OS_PARAM_NAME,
                self.iso.path,
                err.status()
            );
        }

        let virtual_device = self.create_virtual_block_io(boot_config)?;

        match self.boot_virtual_device(&virtual_device) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "Virtual device boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                );

                if !self.iso.image_format.is_iso() {
                    return Err(err);
                }

                match self.iso.os_type {
                    OsType::Windows | OsType::WinPE => self.boot_windows(&virtual_device),
                    OsType::Ubuntu
                    | OsType::Debian
                    | OsType::Fedora
                    | OsType::Arch
                    | OsType::Linux => self.boot_linux(&virtual_device),
                    OsType::Unknown => self.boot_generic(&virtual_device),
                }
            }
        }
    }

    fn boot_efi_executable(&self) -> uefi::Result<()> {
        info!("Booting selected EFI executable: {}", self.iso.path);
        let device_path = self.handle_device_path_bytes(self.iso.volume_handle)?;
        self.load_image_from_device_path(
            self.iso.volume_handle,
            &device_path,
            &self.iso.path,
            "selected EFI file",
        )
    }

    fn prepare_wimboot(&self) -> uefi::Result<()> {
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

    fn prepare_windows_iso_wimboot(&self) -> uefi::Result<()> {
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

    fn collect_wimboot_internal_files(
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

    fn register_wimboot_runtime_files(
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

    fn load_wimboot_helper(&self) -> uefi::Result<SourceVolumeFile> {
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

    fn boot_virtual_config(&self) -> VirtualDeviceConfig {
        use nextboot_virtio::CdRomBootInfo;

        let device_type = if self.iso.image_format.is_iso() {
            match self.iso.os_type {
                OsType::Windows | OsType::WinPE => VirtualDeviceType::DvdRom,
                _ => VirtualDeviceType::HardDisk,
            }
        } else {
            VirtualDeviceType::HardDisk
        };
        let virtual_block_size = if let Some(block_size) = self.iso.virtual_block_size {
            block_size
        } else if self.iso.image_format.uses_512_byte_virtual_sectors() {
            512
        } else {
            match device_type {
                VirtualDeviceType::DvdRom => 2048,
                _ => self.iso.block_size,
            }
        };

        let mut config = VirtualDeviceConfig::new(
            device_type,
            self.iso.start_lba,
            self.iso.virtual_size,
            virtual_block_size,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        if let Some(boot) = self.iso.boot_info {
            config = config.with_cdrom_boot(CdRomBootInfo::new(
                boot.boot_entry,
                u64::from(boot.image_lba),
                boot.image_block_count,
            ));
            info!(
                "Using EFI El Torito boot image: catalog LBA {}, entry {}, image LBA {}, blocks {}",
                boot.catalog_lba, boot.boot_entry, boot.image_lba, boot.image_block_count
            );
        } else if self.iso.image_format.is_iso() && matches!(device_type, VirtualDeviceType::DvdRom)
        {
            warn!("No EFI El Torito boot image found for {}", self.iso.path);
        }

        config
    }

    fn iso9660_virtual_config(&self) -> VirtualDeviceConfig {
        VirtualDeviceConfig::new(
            VirtualDeviceType::DvdRom,
            self.iso.start_lba,
            self.iso.size,
            2048,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path)
    }

    fn build_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        if self
            .iso
            .ventoy_plugin
            .as_ref()
            .is_some_and(|plugin| plugin.auto_memdisk)
        {
            return self.build_auto_memdisk_block_io(config, source_block_io);
        }

        self.build_source_backed_virtual_block_io(config, source_block_io)
    }

    fn build_source_backed_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let mut vbio = if self.iso.image_format == ImageFormat::DynamicVhd {
            self.build_dynamic_vhd_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vhdx {
            self.build_vhdx_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vdi {
            self.build_vdi_block_io(config, source_block_io)?
        } else if self.iso.extents.is_empty() {
            warn!(
                "No extent map for {}, falling back to contiguous LBA {}",
                self.iso.path, self.iso.start_lba
            );
            VirtualBlockIo::new(config)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = SourceVolumeReader::new(source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);

        Ok(vbio)
    }

    fn build_auto_memdisk_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let image_size =
            usize::try_from(config.iso_size).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut image = Vec::new();
        image
            .try_reserve_exact(image_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        image.resize(image_size, 0);

        let source_vbio =
            self.build_source_backed_virtual_block_io(config.clone(), source_block_io)?;
        vhd::read_file_bytes(&source_vbio, 0, &mut image)?;

        info!(
            "Using Ventoy auto_memdisk for {} ({} bytes loaded)",
            self.iso.path, image_size
        );

        let mut vbio = VirtualBlockIo::new(config);
        vbio.set_physical_reader(ZeroPhysicalReader);
        vbio.add_memory_overlay(MemoryOverlay::new(0, image))
            .map_err(virtio_error_to_uefi_status)?;
        Ok(vbio)
    }

    fn build_dynamic_vhd_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let mut footer = [0u8; vhd::FOOTER_SIZE];
        let footer_offset = self
            .iso
            .size
            .checked_sub(vhd::FOOTER_SIZE as u64)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        vhd::read_file_bytes(&file_vbio, footer_offset, &mut footer)?;

        let footer = vhd::parse_dynamic_footer(&footer).ok_or(uefi::Status::LOAD_ERROR)?;
        let virtual_size = config.iso_size;
        if footer.virtual_size != virtual_size {
            warn!(
                "Dynamic VHD virtual size mismatch for {}: scanner={} footer={}",
                self.iso.path, virtual_size, footer.virtual_size
            );
        }

        let mut header = alloc::vec![0u8; vhd::DYNAMIC_HEADER_SIZE];
        vhd::read_file_bytes(&file_vbio, footer.data_offset, &mut header)?;
        let header = vhd::parse_dynamic_header(&header).ok_or(uefi::Status::LOAD_ERROR)?;
        if header.header_version != 0x0001_0000 {
            warn!(
                "Dynamic VHD header version for {} is 0x{:08x}",
                self.iso.path, header.header_version
            );
        }

        let block_size = u64::from(header.block_size);
        if virtual_size == 0 || block_size == 0 || block_size % vhd::SECTOR_SIZE != 0 {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let sectors_per_block = block_size / vhd::SECTOR_SIZE;
        let bitmap_bytes = div_round_up(sectors_per_block, 8)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let entries_needed =
            div_round_up(virtual_size, block_size).ok_or(uefi::Status::LOAD_ERROR)?;
        if entries_needed == 0 || u64::from(header.max_table_entries) < entries_needed {
            return Err(uefi::Status::LOAD_ERROR.into());
        }
        let entries_to_scan = entries_needed;

        let bat_bytes = entries_to_scan
            .checked_mul(4)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        vhd::read_file_bytes(&file_vbio, header.table_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;

        for index in 0..entries_to_scan {
            let bat_offset = usize::try_from(index.checked_mul(4).ok_or(uefi::Status::LOAD_ERROR)?)
                .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let bat_entry = vhd::read_be_u32(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            if bat_entry == vhd::UNUSED_BAT_ENTRY {
                continue;
            }

            let virtual_start = index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= virtual_size {
                break;
            }
            let byte_count = block_size.min(virtual_size - virtual_start);
            let file_offset = u64::from(bat_entry)
                .checked_mul(vhd::SECTOR_SIZE)
                .and_then(|offset| offset.checked_add(bitmap_bytes))
                .ok_or(uefi::Status::LOAD_ERROR)?;

            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(virtual_size);
        byte_mapping.optimize();
        info!(
            "Mapped dynamic VHD {}: virtual={} bytes, block={} bytes, allocated BAT entries={}, physical segments={}",
            self.iso.path,
            virtual_size,
            block_size,
            allocated_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vhdx_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let (regions, metadata) = self.read_vhdx_layout(&file_vbio)?;
        if metadata.has_parent {
            warn!(
                "VHDX parent chains are not supported yet: {}",
                self.iso.path
            );
            return Err(uefi::Status::UNSUPPORTED.into());
        }

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.logical_sector_size;

        let block_size = u64::from(metadata.block_size);
        let payload_blocks =
            vhdx::payload_block_count(metadata.virtual_disk_size, metadata.block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
        let chunk_ratio = metadata.chunk_ratio().ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_entries =
            vhdx::bat_entry_count(payload_blocks, chunk_ratio).ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_bytes = bat_entries
            .checked_mul(8)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;

        if bat_bytes > regions.bat_length {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        vhd::read_file_bytes(&file_vbio, regions.bat_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for payload_index in 0..payload_blocks {
            let bat_index = vhdx::payload_bat_index(payload_index, chunk_ratio)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let bat_offset =
                usize::try_from(bat_index.checked_mul(8).ok_or(uefi::Status::LOAD_ERROR)?)
                    .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let raw_entry = vhdx::read_le_u64(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            let entry = vhdx::parse_bat_entry(raw_entry);
            let virtual_start = payload_index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);

            match entry.state {
                vhdx::VHDX_BAT_STATE_FULLY_PRESENT => {
                    if entry
                        .file_offset
                        .checked_add(byte_count)
                        .map_or(true, |end| end > self.iso.size)
                    {
                        return Err(uefi::Status::DEVICE_ERROR.into());
                    }

                    self.map_image_file_range_to_physical(
                        &mut byte_mapping,
                        virtual_start,
                        entry.file_offset,
                        byte_count,
                    )?;
                    allocated_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_NOT_PRESENT
                | vhdx::VHDX_BAT_STATE_ZERO
                | vhdx::VHDX_BAT_STATE_UNMAPPED => {
                    zero_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_UNDEFINED | vhdx::VHDX_BAT_STATE_PARTIALLY_PRESENT => {
                    warn!(
                        "Unsupported VHDX BAT state {} at payload block {} in {}",
                        entry.state, payload_index, self.iso.path
                    );
                    return Err(uefi::Status::UNSUPPORTED.into());
                }
                _ => {
                    return Err(uefi::Status::LOAD_ERROR.into());
                }
            }
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VHDX {}: virtual={} bytes, block={} bytes, logical_sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.logical_sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vdi_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let metadata = self.read_vdi_metadata(&file_vbio)?;

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.sector_size;

        let map_bytes =
            vdi::block_map_bytes(metadata.block_count).ok_or(uefi::Status::LOAD_ERROR)?;
        if metadata
            .offset_blocks
            .checked_add(map_bytes)
            .map_or(true, |end| {
                end > self.iso.size || end > metadata.offset_data
            })
        {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let map_len = usize::try_from(map_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut block_map = Vec::new();
        block_map
            .try_reserve_exact(map_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        block_map.resize(map_len, 0);
        vhd::read_file_bytes(&file_vbio, metadata.offset_blocks, &mut block_map)?;

        let block_size = u64::from(metadata.block_size);
        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for block_index in 0..metadata.block_count {
            let virtual_start = u64::from(block_index)
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= metadata.virtual_disk_size {
                break;
            }

            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);
            let map_entry = vdi::read_block_map_entry(&block_map, block_index)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if !vdi::is_allocated_block(map_entry) {
                zero_blocks += 1;
                continue;
            }

            if map_entry >= metadata.block_count {
                warn!(
                    "Invalid VDI block map entry {} at virtual block {} in {}",
                    map_entry, block_index, self.iso.path
                );
                return Err(uefi::Status::LOAD_ERROR.into());
            }

            let file_offset = metadata
                .offset_data
                .checked_add(
                    u64::from(map_entry)
                        .checked_mul(block_size)
                        .ok_or(uefi::Status::LOAD_ERROR)?,
                )
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VDI {}: virtual={} bytes, block={} bytes, sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn read_vhdx_layout(
        &self,
        file_vbio: &VirtualBlockIo,
    ) -> uefi::Result<(vhdx::VhdxRegions, vhdx::VhdxMetadata)> {
        let mut header = Vec::new();
        header
            .try_reserve_exact(vhdx::VHDX_HEADER_SECTION_SIZE)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        header.resize(vhdx::VHDX_HEADER_SECTION_SIZE, 0);
        vhd::read_file_bytes(file_vbio, 0, &mut header)?;
        let regions = vhdx::parse_vhdx_regions(&header).ok_or(uefi::Status::LOAD_ERROR)?;

        let metadata_len =
            usize::try_from(regions.metadata_length).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut metadata = Vec::new();
        metadata
            .try_reserve_exact(metadata_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        metadata.resize(metadata_len, 0);
        vhd::read_file_bytes(file_vbio, regions.metadata_offset, &mut metadata)?;
        let metadata = vhdx::parse_vhdx_metadata(&metadata).ok_or(uefi::Status::LOAD_ERROR)?;

        Ok((regions, metadata))
    }

    fn read_vdi_metadata(&self, file_vbio: &VirtualBlockIo) -> uefi::Result<vdi::VdiMetadata> {
        let mut header = [0u8; vdi::VDI_HEADER_SIZE];
        vhd::read_file_bytes(file_vbio, 0, &mut header)?;
        vdi::parse_vdi_metadata(&header).ok_or(uefi::Status::LOAD_ERROR.into())
    }

    fn build_image_file_block_io(&self, source_block_io: &BlockIO) -> uefi::Result<VirtualBlockIo> {
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::HardDisk,
            self.iso.start_lba,
            self.iso.size,
            vhd::SECTOR_SIZE as u32,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        let mut vbio = if self.iso.extents.is_empty() {
            let source_block_size = u64::from(self.iso.block_size);
            let block_count =
                div_round_up(self.iso.size, source_block_size).ok_or(uefi::Status::LOAD_ERROR)?;
            let extents = [(0, self.iso.start_lba, block_count)];
            VirtualBlockIo::from_file_extents(config, &extents)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = SourceVolumeReader::new(source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);
        Ok(vbio)
    }

    fn map_iso_file_extents_to_source_segments(
        &self,
        iso_block_size: u32,
        file_size: u64,
        extents: &[FileExtent],
    ) -> uefi::Result<Vec<WimbootMappedSegment>> {
        if iso_block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }
        if file_size == 0 {
            return Ok(Vec::new());
        }
        if extents.is_empty() {
            return Err(Status::UNSUPPORTED.into());
        }

        let iso_block_size = u64::from(iso_block_size);
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(extents.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;

        for extent in extents {
            let file_virtual_start = extent
                .virtual_block_start
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;
            if file_virtual_start >= file_size {
                continue;
            }

            let extent_bytes = extent
                .block_count
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;
            let byte_count = extent_bytes.min(file_size - file_virtual_start);
            let iso_file_offset = extent
                .physical_lba
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;

            self.append_iso_file_range_to_source_segments(
                &mut segments,
                file_virtual_start,
                iso_file_offset,
                byte_count,
            )?;
        }

        if segments.is_empty() {
            Err(Status::DEVICE_ERROR.into())
        } else {
            Ok(segments)
        }
    }

    fn append_iso_file_range_to_source_segments(
        &self,
        segments: &mut Vec<WimbootMappedSegment>,
        virtual_start: u64,
        iso_file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        if byte_count == 0 {
            return Ok(());
        }

        let source_block_size = u64::from(self.iso.block_size);
        if source_block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }

        if self.iso.extents.is_empty() {
            let physical_offset = self
                .iso
                .start_lba
                .checked_mul(source_block_size)
                .and_then(|start| start.checked_add(iso_file_offset))
                .ok_or(Status::LOAD_ERROR)?;
            segments
                .try_reserve_exact(1)
                .map_err(|_| Status::OUT_OF_RESOURCES)?;
            segments.push(WimbootMappedSegment {
                virtual_offset: virtual_start,
                physical_offset,
                byte_count,
            });
            return Ok(());
        }

        let file_end = iso_file_offset
            .checked_add(byte_count)
            .ok_or(Status::LOAD_ERROR)?;
        let mut cursor = iso_file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &self.iso.extents {
                let extent_file_start = extent
                    .virtual_block_start
                    .checked_mul(source_block_size)
                    .ok_or(Status::LOAD_ERROR)?;
                let extent_bytes = extent
                    .block_count
                    .checked_mul(source_block_size)
                    .ok_or(Status::LOAD_ERROR)?;
                let extent_file_end = extent_file_start
                    .checked_add(extent_bytes)
                    .ok_or(Status::LOAD_ERROR)?;

                if cursor < extent_file_start || cursor >= extent_file_end {
                    continue;
                }

                let overlap_end = file_end.min(extent_file_end);
                let overlap_len = overlap_end - cursor;
                let segment_virtual_start = virtual_start
                    .checked_add(cursor - iso_file_offset)
                    .ok_or(Status::LOAD_ERROR)?;
                let physical_offset = extent
                    .physical_lba
                    .checked_mul(source_block_size)
                    .and_then(|start| start.checked_add(cursor - extent_file_start))
                    .ok_or(Status::LOAD_ERROR)?;

                segments
                    .try_reserve_exact(1)
                    .map_err(|_| Status::OUT_OF_RESOURCES)?;
                segments.push(WimbootMappedSegment {
                    virtual_offset: segment_virtual_start,
                    physical_offset,
                    byte_count: overlap_len,
                });
                cursor = overlap_end;
                mapped = true;
                break;
            }

            if !mapped {
                return Err(Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }

    fn map_image_file_range_to_physical(
        &self,
        table: &mut ByteMappingTable,
        virtual_start: u64,
        file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let source_block_size = u64::from(self.iso.block_size);
        if source_block_size == 0 {
            return Err(uefi::Status::INVALID_PARAMETER.into());
        }

        if self.iso.extents.is_empty() {
            let physical_start = self
                .iso
                .start_lba
                .checked_mul(source_block_size)
                .and_then(|start| start.checked_add(file_offset))
                .ok_or(uefi::Status::LOAD_ERROR)?;
            table.add_segment(virtual_start, byte_count, physical_start);
            return Ok(());
        }

        let file_end = file_offset
            .checked_add(byte_count)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let mut cursor = file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &self.iso.extents {
                let extent_file_start = extent
                    .virtual_block_start
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_bytes = extent
                    .block_count
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_file_end = extent_file_start
                    .checked_add(extent_bytes)
                    .ok_or(uefi::Status::LOAD_ERROR)?;

                if cursor < extent_file_start || cursor >= extent_file_end {
                    continue;
                }

                let overlap_end = file_end.min(extent_file_end);
                let overlap_len = overlap_end - cursor;
                let physical_start = extent
                    .physical_lba
                    .checked_mul(source_block_size)
                    .and_then(|start| start.checked_add(cursor - extent_file_start))
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let segment_virtual_start = virtual_start
                    .checked_add(cursor - file_offset)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                table.add_segment(segment_virtual_start, overlap_len, physical_start);
                cursor = overlap_end;
                mapped = true;
                break;
            }

            if !mapped {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }
}

struct VirtualBootDevice {
    handle: Handle,
    device_path: Vec<u8>,
}

/// 引导模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// 直接内核启动
    DirectKernel,
    /// EFI 链式加载
    ChainLoad,
    /// 虚拟设备引导
    VirtualDevice,
    /// 内存引导
    MemDisk,
}

impl Default for BootMode {
    fn default() -> Self {
        BootMode::VirtualDevice
    }
}

/// 引导选项
#[derive(Debug, Clone)]
pub struct BootOptions {
    /// 引导模式
    pub mode: BootMode,
    /// 内核参数
    pub kernel_args: String,
    /// 是否启用调试
    pub debug: bool,
    /// 超时 (秒)
    pub timeout: Option<u64>,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            mode: BootMode::default(),
            kernel_args: String::new(),
            debug: false,
            timeout: None,
        }
    }
}

/// 内存映射信息
#[derive(Debug, Clone)]
pub struct MemoryMapInfo {
    /// 起始地址
    pub start: u64,
    /// 大小
    pub size: u64,
    /// 类型
    pub memory_type: u32,
}

/// 分配引导内存
pub fn allocate_boot_memory(bt: &BootServices, size: usize) -> uefi::Result<*mut u8> {
    use uefi::table::boot::MemoryType;

    let pages = (size + 4095) / 4096;

    bt.allocate_pages(
        uefi::table::boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        pages,
    )
    .map(|addr| addr as *mut u8)
}

/// 释放引导内存
pub fn free_boot_memory(bt: &BootServices, ptr: *mut u8, size: usize) -> uefi::Result<()> {
    let pages = (size + 4095) / 4096;
    unsafe { bt.free_pages(ptr as u64, pages) }
}
