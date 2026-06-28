use super::block_io::{UefiBlockIo, VirtualIsoBlockIo};
use super::helpers::{default_virtual_block_size, parse_vhd_footer};
use super::model::{ImageFormat, IsoBootInfo, IsoExtent, OsType, WimBootInfo};
use super::IsoScanner;
use crate::{vdi, vhdx, wim};
use alloc::rc::Rc;
use alloc::vec::Vec;
use nextboot_fs::iso9660::{detect_udf_volume, read_efi_eltorito_boot_info, Iso9660};
use nextboot_fs::udf::Udf;
use nextboot_fs::FileSystem;
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::media::block::BlockIO;

const WINDOWS_ISO_MARKERS: &[&str] = &[
    "/sources/boot.wim",
    "/sources/install.wim",
    "/sources/install.esd",
    "/sources/install.swm",
    "/efi/microsoft/boot/bootmgfw.efi",
    "/efi/microsoft/boot/bcd",
    "/boot/bcd",
];

const LINUX_ISO_MARKERS: &[&str] = &[
    "/boot/grub/grub.cfg",
    "/boot/grub/loopback.cfg",
    "/isolinux/isolinux.cfg",
    "/syslinux/syslinux.cfg",
    "/casper/vmlinuz",
    "/live/vmlinuz",
    "/images/pxeboot/vmlinuz",
    "/arch/boot/x86_64/vmlinuz-linux",
];

impl<'a> IsoScanner<'a> {
    pub(super) fn detect_image_virtual_metadata(
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

    pub(super) fn read_wim_boot_info(
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
        let extent_map = extent_map(extents);
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
        let extent_map = extent_map(extents);
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        vbio.set_physical_reader(UefiBlockIo::new(block_io)?);

        let mut data = alloc::vec![0u8; len];
        vbio.read_bytes(vbio.media_id(), offset, &mut data).ok()?;
        Some(data)
    }

    pub(super) fn resolve_iso_metadata(
        &self,
        block_io: &BlockIO,
        source_block_size: u32,
        size: u64,
        extents: &[IsoExtent],
    ) -> (Option<IsoBootInfo>, bool, Option<OsType>) {
        if extents.is_empty() || size == 0 {
            return (None, false, None);
        }

        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, size, 2048)
            .with_physical_block_size(source_block_size);
        let extent_map = extent_map(extents);

        let mut vbio = VirtualBlockIo::from_file_extents(config, &extent_map);
        let Some(reader) = UefiBlockIo::new(block_io) else {
            return (None, false, None);
        };
        vbio.set_physical_reader(reader);
        let iso_io = VirtualIsoBlockIo::new(vbio);

        let boot_info = read_efi_eltorito_boot_info(&iso_io)
            .ok()
            .flatten()
            .map(IsoBootInfo::from);
        let is_udf = detect_udf_volume(&iso_io).unwrap_or(false);
        let os_type_hint = detect_iso_os_type(iso_io, is_udf);

        (boot_info, is_udf, os_type_hint)
    }
}

fn detect_iso_os_type(iso_io: VirtualIsoBlockIo, is_udf: bool) -> Option<OsType> {
    let shared: nextboot_fs::SharedBlockIo = Rc::new(iso_io);

    if is_udf {
        if let Ok(udf) = Udf::open(shared.clone()) {
            if filesystem_has_any_marker(&udf, WINDOWS_ISO_MARKERS) {
                return Some(OsType::Windows);
            }
            if filesystem_has_any_marker(&udf, LINUX_ISO_MARKERS) {
                return Some(OsType::Linux);
            }
        }
    }

    let iso = Iso9660::open(shared).ok()?;
    if filesystem_has_any_marker(&iso, WINDOWS_ISO_MARKERS) {
        return Some(OsType::Windows);
    }
    if filesystem_has_any_marker(&iso, LINUX_ISO_MARKERS) {
        return Some(OsType::Linux);
    }

    None
}

fn filesystem_has_any_marker<F: FileSystem>(fs: &F, paths: &[&str]) -> bool {
    paths.iter().any(|path| fs.stat(path).is_ok())
}

fn extent_map(extents: &[IsoExtent]) -> Vec<(u64, u64, u64)> {
    extents
        .iter()
        .map(|extent| {
            (
                extent.virtual_block_start,
                extent.physical_lba,
                extent.block_count,
            )
        })
        .collect()
}
