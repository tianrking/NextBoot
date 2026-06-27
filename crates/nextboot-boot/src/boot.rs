//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::scanner::{ImageFormat, IsoFile, OsType};
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_fs::FileExtent;
use nextboot_virtio::{MemoryOverlay, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::BootServices;
use uefi::table::runtime::RuntimeServices;
use uefi::{Handle, Status};

mod candidates;
mod chain_load;
mod errors;
mod file_access;
mod image_backing;
mod linux;
mod linux_ventoy;
mod load_file;
mod model;
mod os_param;
mod source_volume;
mod util;
mod vhd;
mod virtual_boot;
mod virtual_device;
mod wimboot_flow;
mod wimboot_resources;
mod wimboot_runtime;
use errors::virtio_error_to_uefi_status;
#[allow(unused_imports)]
pub use model::{
    allocate_boot_memory, free_boot_memory, BootMode, BootOptions, MemoryMapInfo,
};
use model::VirtualBootDevice;
use source_volume::{SourceVolumeReader, ZeroPhysicalReader};
use wimboot_runtime::WimbootMappedSegment;

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
}
