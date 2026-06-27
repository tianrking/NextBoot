use super::block_io::{UefiBlockIo, VirtualIsoBlockIo};
use super::helpers::{
    default_virtual_block_size, offset_extents_for_physical_read, parse_vhd_footer,
};
use super::model::{
    ImageFormat, IsoBootInfo, IsoExtent, ResolvedImageMetadata, VolumeBlockInfo, WimBootInfo,
};
use super::{block_io_info, device_path_to_vec, source_file_extents_from_detected_fs, IsoScanner};
use crate::source_disk::{
    build_source_disk_identity, parent_device_path_bytes, parse_last_hard_drive_device_path,
    SourceDiskIdentity,
};
use crate::{vdi, vhdx, wim};
use alloc::rc::Rc;
use alloc::vec::Vec;
use nextboot_fs::iso9660::{detect_udf_volume, read_efi_eltorito_boot_info};
use nextboot_fs::{detect_fs_type, BlockIoOps, FileSystem};
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::Handle;

impl<'a> IsoScanner<'a> {
    pub(super) fn volume_block_info(&self, volume_handle: Handle) -> Option<VolumeBlockInfo> {
        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(volume_handle)
            .ok()?;
        block_io_info(&block_io)
    }

    pub(super) fn resolve_image_metadata(
        &self,
        volume_handle: Handle,
        path: &str,
        size: u64,
        image_format: ImageFormat,
    ) -> Option<ResolvedImageMetadata> {
        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(volume_handle)
            .ok()?;
        let media = block_io.media();
        if !media.is_media_present() {
            return None;
        }

        let block_size = media.block_size();
        let uefi_io = UefiBlockIo::new(&block_io)?;
        let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
        let mut boot_sector = alloc::vec![0u8; block_size as usize];
        shared.read_blocks(0, &mut boot_sector).ok()?;

        let (block_size, extents) =
            source_file_extents_from_detected_fs(shared, detect_fs_type(&boot_sector), path)?;

        let extents: Vec<IsoExtent> = extents.into_iter().map(IsoExtent::from).collect();
        let (image_format, virtual_size, virtual_block_size) =
            self.detect_image_virtual_metadata(&block_io, block_size, size, &extents, image_format);
        let (boot_info, is_udf) = if image_format.is_iso() {
            self.resolve_iso_metadata(&block_io, block_size, size, &extents)
        } else {
            (None, false)
        };
        let wim_info = if image_format.is_wim_container() {
            self.read_wim_boot_info(&block_io, block_size, size, &extents)
        } else {
            None
        };

        Some(ResolvedImageMetadata {
            block_size,
            extents,
            boot_info,
            is_udf,
            wim_info,
            image_format,
            virtual_size,
            virtual_block_size,
        })
    }

    pub(super) fn resolve_block_image_metadata<F: FileSystem>(
        &self,
        block_io: &BlockIO,
        fs: &F,
        path: &str,
        size: u64,
        image_format: ImageFormat,
        extent_lba_offset: u64,
    ) -> Option<ResolvedImageMetadata> {
        let block_size = fs.block_size();
        let extents: Vec<IsoExtent> = fs
            .file_extents(path)
            .ok()?
            .into_iter()
            .map(IsoExtent::from)
            .collect();
        let read_extents = offset_extents_for_physical_read(&extents, extent_lba_offset)?;
        let (image_format, virtual_size, virtual_block_size) = self.detect_image_virtual_metadata(
            block_io,
            block_size,
            size,
            &read_extents,
            image_format,
        );
        let (boot_info, is_udf) = if image_format.is_iso() {
            self.resolve_iso_metadata(block_io, block_size, size, &read_extents)
        } else {
            (None, false)
        };
        let wim_info = if image_format.is_wim_container() {
            self.read_wim_boot_info(block_io, block_size, size, &read_extents)
        } else {
            None
        };

        Some(ResolvedImageMetadata {
            block_size,
            extents,
            boot_info,
            is_udf,
            wim_info,
            image_format,
            virtual_size,
            virtual_block_size,
        })
    }

    pub(super) fn resolve_source_disk_identity(
        &self,
        volume_handle: Handle,
    ) -> Option<SourceDiskIdentity> {
        let volume_device_path = self.handle_device_path_bytes(volume_handle);
        let hard_drive = volume_device_path
            .as_deref()
            .and_then(parse_last_hard_drive_device_path);
        let parent_handle = match (volume_device_path.as_deref(), hard_drive.as_ref()) {
            (Some(path), Some(info)) => self.locate_parent_block_io(path, info)?,
            (_, None) => volume_handle,
            _ => return None,
        };

        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(parent_handle)
            .ok()?;
        let media = block_io.media();
        if hard_drive.is_none() && media.is_logical_partition() {
            return None;
        }

        let block_size = media.block_size();
        if block_size < 512 {
            return None;
        }
        let total_blocks = media.last_block().checked_add(1)?;
        let disk_size = total_blocks.checked_mul(u64::from(block_size))?;
        let block_len = usize::try_from(block_size).ok()?;
        let mut first_block = Vec::new();
        first_block.try_reserve_exact(block_len).ok()?;
        first_block.resize(block_len, 0);
        block_io
            .read_blocks(media.media_id(), 0, &mut first_block)
            .ok()?;

        build_source_disk_identity(&first_block, disk_size, block_size, hard_drive)
    }

    fn locate_parent_block_io(
        &self,
        volume_device_path: &[u8],
        hard_drive: &crate::source_disk::HardDriveDevicePathInfo,
    ) -> Option<Handle> {
        let parent_path = parent_device_path_bytes(volume_device_path, hard_drive)?;
        let mut device_path =
            unsafe { DevicePath::from_ffi_ptr(parent_path.as_ptr().cast::<FfiDevicePath>()) };
        self.bt.locate_device_path::<BlockIO>(&mut device_path).ok()
    }

    pub(super) fn handle_device_path_bytes(&self, handle: Handle) -> Option<Vec<u8>> {
        let device_path = self.bt.open_protocol_exclusive::<DevicePath>(handle).ok()?;
        device_path_to_vec(&device_path)
    }

    fn detect_image_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        image_format: ImageFormat,
    ) -> (ImageFormat, u64, Option<u32>) {
        match image_format {
            ImageFormat::Vhd => {
                match self.read_image_tail(block_io, source_block_size, file_size, extents, 512) {
                    Some(footer) => parse_vhd_footer(&footer)
                        .map(|info| {
                            let virtual_size = if info.image_format == ImageFormat::FixedVhd {
                                info.virtual_size.min(file_size.saturating_sub(512))
                            } else {
                                info.virtual_size
                            };
                            (info.image_format, virtual_size, Some(512))
                        })
                        .unwrap_or((
                            image_format,
                            file_size,
                            default_virtual_block_size(image_format),
                        )),
                    None => (
                        image_format,
                        file_size,
                        default_virtual_block_size(image_format),
                    ),
                }
            }
            ImageFormat::Vhdx => self
                .read_vhdx_virtual_metadata(block_io, source_block_size, file_size, extents)
                .map(|metadata| {
                    (
                        image_format,
                        metadata.virtual_disk_size,
                        Some(metadata.logical_sector_size),
                    )
                })
                .unwrap_or((
                    image_format,
                    file_size,
                    default_virtual_block_size(image_format),
                )),
            ImageFormat::Vdi => self
                .read_vdi_virtual_metadata(block_io, source_block_size, file_size, extents)
                .map(|metadata| {
                    (
                        image_format,
                        metadata.virtual_disk_size,
                        Some(metadata.sector_size),
                    )
                })
                .unwrap_or((
                    image_format,
                    file_size,
                    default_virtual_block_size(image_format),
                )),
            _ => (
                image_format,
                file_size,
                default_virtual_block_size(image_format),
            ),
        }
    }

    fn read_vhdx_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<vhdx::VhdxMetadata> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            vhdx::VHDX_HEADER_SECTION_SIZE,
        )?;
        let regions = vhdx::parse_vhdx_regions(&header)?;
        if regions.metadata_length > usize::MAX as u64 {
            return None;
        }
        let metadata = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            regions.metadata_offset,
            regions.metadata_length as usize,
        )?;
        vhdx::parse_vhdx_metadata(&metadata)
    }

    fn read_vdi_virtual_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<vdi::VdiMetadata> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            vdi::VDI_HEADER_SIZE,
        )?;
        vdi::parse_vdi_metadata(&header)
    }

    fn read_wim_boot_info(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
    ) -> Option<WimBootInfo> {
        let header = self.read_image_bytes(
            block_io,
            source_block_size,
            file_size,
            extents,
            0,
            wim::WIM_HEADER_SIZE,
        )?;
        wim::parse_wim_metadata(&header).map(WimBootInfo::from)
    }

    fn read_image_tail(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        tail_len: usize,
    ) -> Option<Vec<u8>> {
        if extents.is_empty() || tail_len == 0 || file_size < tail_len as u64 {
            return None;
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, file_size, 512)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);

        let offset = file_size.checked_sub(tail_len as u64)?;
        if offset % 512 != 0 {
            return None;
        }

        let mut data = alloc::vec![0u8; tail_len];
        vbio.read_blocks(vbio.media_id(), offset / 512, &mut data)
            .ok()?;
        Some(data)
    }

    fn read_image_bytes(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        file_size: u64,
        extents: &[IsoExtent],
        offset: u64,
        len: usize,
    ) -> Option<Vec<u8>> {
        if extents.is_empty() || len == 0 {
            return None;
        }

        let end = offset.checked_add(len as u64)?;
        if end > file_size {
            return None;
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, file_size, 512)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);

        let mut data = alloc::vec![0u8; len];
        vbio.read_bytes(vbio.media_id(), offset, &mut data).ok()?;
        Some(data)
    }

    fn resolve_iso_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        size: u64,
        extents: &[IsoExtent],
    ) -> (Option<IsoBootInfo>, bool) {
        if extents.is_empty() || size == 0 {
            return (None, false);
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, size, 2048)
            .with_physical_block_size(source_block_size);
        let extent_map: Vec<(u64, u64, u64)> = extents
            .iter()
            .map(|extent| {
                (
                    extent.virtual_block_start,
                    extent.physical_lba,
                    extent.block_count,
                )
            })
            .collect();

        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        let Some(reader) = UefiBlockIo::new(block_io) else {
            return (None, false);
        };
        vbio.set_physical_reader(reader);
        let iso_io = VirtualIsoBlockIo::new(vbio);

        let boot_info = read_efi_eltorito_boot_info(&iso_io)
            .ok()
            .flatten()
            .map(IsoBootInfo::from);
        let is_udf = detect_udf_volume(&iso_io).unwrap_or(false);

        (boot_info, is_udf)
    }
}
