use super::errors::{fs_error_to_uefi_status, ventoy_error_to_uefi_status};
use super::source_volume::SourceVolumeReader;
use super::util::{
    align_up, div_round_up, os_type_code, push_extent_record, push_u16, push_u32, push_u64,
    runtime_extent_count, usize_to_u16, usize_to_u32, ventoy_chain_type, virtual_device_type_code,
};
use super::BootManager;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ptr;
use log::{info, warn};
use nextboot_fs::{detect_fs_type, BlockIoOps, FileSystemType, SharedBlockIo};
use nextboot_virtio::VirtualDeviceConfig;
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::MemoryType;
use uefi::table::runtime::{VariableAttributes, VariableVendor};
use uefi::{CString16, Guid, Status};

pub(super) const NEXTBOOT_OS_PARAM_NAME: &str = "NextBootOsParam";
const NEXTBOOT_OS_PARAM_VENDOR_GUID: Guid = uefi::guid!("c1775af2-4211-4f55-9f6f-2cc5ef5667f0");
const VENTOY_OS_PARAM_VENDOR_GUID: Guid = uefi::guid!("77772020-2e77-6576-6e74-6f792e6e6574");
const NEXTBOOT_OS_PARAM_MAGIC: &[u8; 8] = b"NBOSPARM";
const NEXTBOOT_OS_PARAM_VERSION: u16 = 1;
const NEXTBOOT_OS_PARAM_HEADER_SIZE: usize = 80;
const NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE: usize = 24;
const NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT: u16 = 0x0001;
const NEXTBOOT_OS_PARAM_FLAG_EL_TORITO: u16 = 0x0002;
const VENTOY_RUNTIME_ALIGNMENT: usize = 4096;

impl BootManager<'_> {
    pub(super) fn publish_os_param(&self, config: &VirtualDeviceConfig) -> uefi::Result<()> {
        let data = self.build_os_param_payload(config)?;
        let name = CString16::try_from(NEXTBOOT_OS_PARAM_NAME)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        let vendor = VariableVendor(NEXTBOOT_OS_PARAM_VENDOR_GUID);
        let attributes =
            VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.rt
            .set_variable(name.as_ref(), &vendor, attributes, &data)?;
        info!(
            "Published {} ({} bytes, {} extent record(s))",
            NEXTBOOT_OS_PARAM_NAME,
            data.len(),
            runtime_extent_count(self.iso)
        );

        if let Err(err) = self.publish_ventoy_os_param(config) {
            warn!(
                "Failed to publish {} for {}: {:?}",
                crate::ventoy::VENTOY_OS_PARAM_NAME,
                self.iso.path,
                err.status()
            );
        }

        Ok(())
    }

    fn publish_ventoy_os_param(&self, config: &VirtualDeviceConfig) -> uefi::Result<()> {
        let (data, image_region_count, image_location_addr) =
            self.build_ventoy_os_param_payload(config)?;
        let name = CString16::try_from(crate::ventoy::VENTOY_OS_PARAM_NAME)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        let vendor = VariableVendor(VENTOY_OS_PARAM_VENDOR_GUID);
        let attributes =
            VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.rt
            .set_variable(name.as_ref(), &vendor, attributes, &data)?;
        info!(
            "Published {} ({} bytes, {} image location region(s), location=0x{:x})",
            crate::ventoy::VENTOY_OS_PARAM_NAME,
            data.len(),
            image_region_count,
            image_location_addr
        );

        Ok(())
    }

    pub(super) fn build_ventoy_os_param_payload(
        &self,
        config: &VirtualDeviceConfig,
    ) -> uefi::Result<([u8; crate::ventoy::VENTOY_OS_PARAM_SIZE], usize, usize)> {
        let (image_sector_size, disk_sector_size, image_regions) =
            self.build_ventoy_image_regions(config)?;
        let image_location = crate::ventoy::build_ventoy_image_location(
            image_sector_size,
            disk_sector_size,
            &image_regions,
        )
        .map_err(ventoy_error_to_uefi_status)?;
        let image_location_addr =
            self.copy_to_runtime_pool_aligned(&image_location, VENTOY_RUNTIME_ALIGNMENT)?;
        let source_disk = self.iso.source_disk;
        let disk_part_type = self
            .detect_ventoy_source_partition_type()
            .unwrap_or(crate::ventoy::VENTOY_PART_TYPE_OTHER);
        let disk_part_id = source_disk
            .and_then(|disk| {
                if disk.partition_number == 0 {
                    None
                } else {
                    Some(disk.partition_number)
                }
            })
            .unwrap_or(usize_to_u16(self.iso.volume_index.saturating_add(1))?);
        let disk_signature = source_disk.map_or([0; 4], |disk| disk.disk_signature);
        let reserved = self.ventoy_reserved_flags(disk_signature);
        let input = crate::ventoy::VentoyOsParamInput {
            disk_guid: source_disk.map_or([0; 16], |disk| disk.disk_guid),
            disk_size: source_disk.map_or(self.iso.source_disk_size, |disk| disk.disk_size),
            disk_part_id,
            disk_part_type,
            image_path: &self.iso.path,
            image_size: self.iso.size,
            image_location_addr: image_location_addr as u64,
            image_location_len: usize_to_u32(image_location.len())?,
            reserved,
            disk_signature,
        };
        let data =
            crate::ventoy::build_ventoy_os_param(&input).map_err(ventoy_error_to_uefi_status)?;
        Ok((data, image_regions.len(), image_location_addr))
    }

    fn ventoy_reserved_flags(&self, disk_signature: [u8; 4]) -> crate::ventoy::VentoyReserved {
        let chain_type = ventoy_chain_type(self.iso);
        let windows_cd_prompt =
            chain_type == crate::ventoy::VENTOY_CHAIN_WINDOWS && self.iso.ventoy_windows_cd_prompt;
        let windows_resolution_lock = if chain_type == crate::ventoy::VENTOY_CHAIN_WINDOWS {
            self.iso.ventoy_windows_uefi_resolution_lock
        } else {
            0
        };

        crate::ventoy::VentoyReserved::new()
            .with_chain_type(chain_type)
            .with_iso_udf(self.iso.is_udf)
            .with_windows_cd_prompt(windows_cd_prompt)
            .with_linux_remount(self.iso.ventoy_linux_remount)
            .with_vlnk(self.iso.is_vlnk)
            .with_disk_signature(disk_signature)
            .with_windows_max_resolution(windows_resolution_lock)
    }

    fn build_ventoy_image_regions(
        &self,
        config: &VirtualDeviceConfig,
    ) -> uefi::Result<(u32, u32, Vec<crate::ventoy::VentoyImageRegion>)> {
        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(self.iso.block_size, |disk| disk.block_size);
        if disk_sector_size != self.iso.block_size {
            warn!(
                "VentoyOsParam source disk sector size {} differs from volume sector size {} for {}",
                disk_sector_size, self.iso.block_size, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let extents = self.ventoy_source_extents()?;
        let preferred_image_sector_size = if self.iso.image_format.is_iso() {
            2048
        } else {
            config.block_size
        };

        match crate::ventoy::build_ventoy_image_regions(
            &extents,
            self.iso.block_size,
            preferred_image_sector_size,
        ) {
            Ok(regions) => Ok((preferred_image_sector_size, disk_sector_size, regions)),
            Err(crate::ventoy::VentoyParamError::UnalignedExtent)
                if preferred_image_sector_size != self.iso.block_size =>
            {
                let regions = crate::ventoy::build_ventoy_image_regions(
                    &extents,
                    self.iso.block_size,
                    self.iso.block_size,
                )
                .map_err(ventoy_error_to_uefi_status)?;
                Ok((self.iso.block_size, disk_sector_size, regions))
            }
            Err(err) => Err(ventoy_error_to_uefi_status(err).into()),
        }
    }

    pub(super) fn ventoy_source_extents(&self) -> uefi::Result<Vec<crate::ventoy::VentoyExtent>> {
        let mut extents = Vec::new();
        let count = if self.iso.extents.is_empty() {
            1
        } else {
            self.iso.extents.len()
        };
        extents
            .try_reserve_exact(count)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        if self.iso.extents.is_empty() {
            let disk_lba_offset = self
                .iso
                .source_disk
                .map_or(0, |disk| disk.partition_start_lba);
            let block_count = div_round_up(self.iso.size, u64::from(self.iso.block_size))
                .ok_or(uefi::Status::INVALID_PARAMETER)?;
            extents.push(crate::ventoy::VentoyExtent {
                virtual_block_start: 0,
                physical_lba: self
                    .iso
                    .start_lba
                    .checked_add(disk_lba_offset)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                block_count,
            });
        } else {
            let disk_lba_offset = self
                .iso
                .source_disk
                .map_or(0, |disk| disk.partition_start_lba);
            for extent in &self.iso.extents {
                extents.push(crate::ventoy::VentoyExtent {
                    virtual_block_start: extent.virtual_block_start,
                    physical_lba: extent
                        .physical_lba
                        .checked_add(disk_lba_offset)
                        .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                    block_count: extent.block_count,
                });
            }
        }

        Ok(extents)
    }

    fn copy_to_runtime_pool_aligned(&self, data: &[u8], alignment: usize) -> uefi::Result<usize> {
        if data.is_empty() || !alignment.is_power_of_two() {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let allocation_size = data
            .len()
            .checked_add(
                alignment
                    .checked_mul(2)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
            )
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let raw = self
            .bt
            .allocate_pool(MemoryType::RUNTIME_SERVICES_DATA, allocation_size)?;
        let aligned = align_up(raw as usize, alignment).ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), aligned as *mut u8, data.len());
        }

        Ok(aligned)
    }

    fn detect_ventoy_source_partition_type(&self) -> uefi::Result<u16> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        let shared: SharedBlockIo = Rc::new(reader);
        let block_size =
            usize::try_from(shared.block_size()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        if block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let mut boot_sector = Vec::new();
        boot_sector
            .try_reserve_exact(block_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        boot_sector.resize(block_size, 0);
        shared
            .read_blocks(0, &mut boot_sector)
            .map_err(fs_error_to_uefi_status)?;

        let fs_type = detect_fs_type(&boot_sector);
        let source_is_udf = matches!(fs_type, FileSystemType::Unknown | FileSystemType::Iso9660)
            && nextboot_fs::udf::Udf::open(shared).is_ok();

        Ok(match fs_type {
            FileSystemType::ExFat => crate::ventoy::VENTOY_PART_TYPE_EXFAT,
            FileSystemType::Fat32 => crate::ventoy::VENTOY_PART_TYPE_FAT,
            FileSystemType::Ntfs => crate::ventoy::VENTOY_PART_TYPE_NTFS,
            _ if source_is_udf => crate::ventoy::VENTOY_PART_TYPE_UDF,
            _ => crate::ventoy::VENTOY_PART_TYPE_OTHER,
        })
    }

    fn build_os_param_payload(&self, config: &VirtualDeviceConfig) -> uefi::Result<Vec<u8>> {
        let path = self.iso.path.as_bytes();
        let extent_count = runtime_extent_count(self.iso);
        let path_offset = NEXTBOOT_OS_PARAM_HEADER_SIZE;
        let path_end = path_offset
            .checked_add(path.len())
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_offset = align_up(path_end, 8).ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_len = extent_count
            .checked_mul(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let total_size = extents_offset
            .checked_add(extents_len)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;

        let mut flags = 0u16;
        if self.iso.extents.is_empty() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT;
        }
        if self.iso.boot_info.is_some() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_EL_TORITO;
        }

        let mut data = Vec::new();
        data.try_reserve_exact(total_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.extend_from_slice(NEXTBOOT_OS_PARAM_MAGIC);
        push_u16(&mut data, NEXTBOOT_OS_PARAM_VERSION);
        push_u16(&mut data, usize_to_u16(NEXTBOOT_OS_PARAM_HEADER_SIZE)?);
        push_u16(
            &mut data,
            usize_to_u16(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)?,
        );
        push_u16(&mut data, flags);
        push_u32(&mut data, usize_to_u32(total_size)?);
        push_u32(&mut data, usize_to_u32(self.iso.volume_index)?);
        push_u32(&mut data, os_type_code(self.iso.os_type));
        push_u32(&mut data, virtual_device_type_code(config.device_type));
        push_u64(&mut data, self.iso.virtual_size);
        push_u64(&mut data, self.iso.start_lba);
        push_u32(&mut data, self.iso.block_size);
        push_u32(&mut data, config.block_size);
        push_u32(&mut data, config.physical_block_size);
        push_u32(&mut data, usize_to_u32(extent_count)?);
        push_u32(&mut data, usize_to_u32(path_offset)?);
        push_u32(&mut data, usize_to_u32(path.len())?);
        push_u32(&mut data, usize_to_u32(extents_offset)?);
        push_u32(&mut data, usize_to_u32(extents_len)?);
        debug_assert_eq!(data.len(), NEXTBOOT_OS_PARAM_HEADER_SIZE);

        data.extend_from_slice(path);
        data.resize(extents_offset, 0);
        self.append_runtime_extents(&mut data)?;
        debug_assert_eq!(data.len(), total_size);

        Ok(data)
    }

    fn append_runtime_extents(&self, data: &mut Vec<u8>) -> uefi::Result<()> {
        if self.iso.extents.is_empty() {
            let block_count = div_round_up(self.iso.virtual_size, u64::from(self.iso.block_size))
                .ok_or(uefi::Status::INVALID_PARAMETER)?;
            push_extent_record(data, 0, self.iso.start_lba, block_count);
            return Ok(());
        }

        for extent in &self.iso.extents {
            push_extent_record(
                data,
                extent.virtual_block_start,
                extent.physical_lba,
                extent.block_count,
            );
        }

        Ok(())
    }
}
