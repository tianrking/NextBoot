use super::{vhd, BootManager};
use crate::vhdx;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig};
use uefi::proto::media::block::BlockIO;

impl BootManager<'_> {
    pub(super) fn build_vhdx_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let (regions, metadata) = self.read_vhdx_layout(&file_vbio)?;
        let parent = self.open_vhdx_parent_backing(source_block_io, &metadata)?;

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.logical_sector_size;

        let block_size = u64::from(metadata.block_size);
        let payload_blocks =
            vhdx::payload_block_count(metadata.virtual_disk_size, metadata.block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
        let chunk_ratio = metadata.chunk_ratio().ok_or(uefi::Status::LOAD_ERROR)?;
        let bat = self.read_vhdx_bat(&file_vbio, &regions, payload_blocks, chunk_ratio)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut parent_blocks = 0u64;
        let mut partial_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for payload_index in 0..payload_blocks {
            let entry = read_payload_bat_entry(&bat, payload_index, chunk_ratio)?;
            let virtual_start = payload_index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);

            match entry.state {
                vhdx::VHDX_BAT_STATE_FULLY_PRESENT => {
                    self.map_vhdx_payload_block(
                        &mut byte_mapping,
                        virtual_start,
                        entry.file_offset,
                        byte_count,
                    )?;
                    allocated_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_PARTIALLY_PRESENT => {
                    if !self.vhdx_partial_block_is_self_contained(
                        &file_vbio,
                        &bat,
                        &metadata,
                        payload_index,
                        chunk_ratio,
                        byte_count,
                    )? {
                        warn!(
                            "VHDX partial block {} in {} still references parent data",
                            payload_index, self.iso.path
                        );
                        return Err(uefi::Status::UNSUPPORTED.into());
                    }
                    self.map_vhdx_payload_block(
                        &mut byte_mapping,
                        virtual_start,
                        entry.file_offset,
                        byte_count,
                    )?;
                    allocated_blocks += 1;
                    partial_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_ZERO => {
                    zero_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_NOT_PRESENT | vhdx::VHDX_BAT_STATE_UNMAPPED => {
                    if metadata.has_parent {
                        let Some(parent) = parent.as_ref() else {
                            warn!(
                                "VHDX block {} in {} requires an unsupported parent chain",
                                payload_index, self.iso.path
                            );
                            return Err(uefi::Status::UNSUPPORTED.into());
                        };
                        self.map_parent_vhdx_range(
                            &mut byte_mapping,
                            parent,
                            virtual_start,
                            byte_count,
                        )?;
                        parent_blocks += 1;
                    } else {
                        zero_blocks += 1;
                    }
                }
                vhdx::VHDX_BAT_STATE_UNDEFINED => {
                    warn!(
                        "Unsupported VHDX undefined BAT state at payload block {} in {}",
                        payload_index, self.iso.path
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
            "Mapped VHDX {}: virtual={} bytes, block={} bytes, logical_sector={} bytes, allocated_blocks={}, parent_blocks={}, partial_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.logical_sector_size,
            allocated_blocks,
            parent_blocks,
            partial_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    pub(super) fn read_vhdx_layout(
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

    pub(super) fn read_vhdx_bat(
        &self,
        file_vbio: &VirtualBlockIo,
        regions: &vhdx::VhdxRegions,
        payload_blocks: u64,
        chunk_ratio: u64,
    ) -> uefi::Result<Vec<u8>> {
        let bat_entries =
            vhdx::bat_entry_count(payload_blocks, chunk_ratio).ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_bytes = bat_entries
            .checked_mul(8)
            .and_then(|bytes| super::util::align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;

        if bat_bytes > regions.bat_length {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        vhd::read_file_bytes(file_vbio, regions.bat_offset, &mut bat)?;
        Ok(bat)
    }

    fn map_vhdx_payload_block(
        &self,
        byte_mapping: &mut ByteMappingTable,
        virtual_start: u64,
        file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        if file_offset
            .checked_add(byte_count)
            .is_none_or(|end| end > self.iso.size)
        {
            return Err(uefi::Status::DEVICE_ERROR.into());
        }
        self.map_image_file_range_to_physical(byte_mapping, virtual_start, file_offset, byte_count)
    }

    fn vhdx_partial_block_is_self_contained(
        &self,
        file_vbio: &VirtualBlockIo,
        bat: &[u8],
        metadata: &vhdx::VhdxMetadata,
        payload_index: u64,
        chunk_ratio: u64,
        byte_count: u64,
    ) -> uefi::Result<bool> {
        let bitmap_entry = read_sector_bitmap_bat_entry(bat, payload_index, chunk_ratio)?;
        if bitmap_entry.state != vhdx::VHDX_BAT_STATE_FULLY_PRESENT {
            return Ok(false);
        }
        if bitmap_entry
            .file_offset
            .checked_add(vhdx::VHDX_MIB)
            .is_none_or(|end| end > self.iso.size)
        {
            return Err(uefi::Status::DEVICE_ERROR.into());
        }

        let mut bitmap = Vec::new();
        bitmap
            .try_reserve_exact(vhdx::VHDX_MIB as usize)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bitmap.resize(vhdx::VHDX_MIB as usize, 0);
        vhd::read_file_bytes(file_vbio, bitmap_entry.file_offset, &mut bitmap)?;

        let sectors_per_block = u64::from(metadata.block_size / metadata.logical_sector_size);
        let chunk_block_index = payload_index % chunk_ratio;
        let first_sector = chunk_block_index
            .checked_mul(sectors_per_block)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let sector_count =
            super::util::div_round_up(byte_count, u64::from(metadata.logical_sector_size))
                .ok_or(uefi::Status::LOAD_ERROR)?;

        Ok(bitmap_range_all_present(
            &bitmap,
            first_sector,
            sector_count,
        ))
    }
}

fn read_payload_bat_entry(
    bat: &[u8],
    payload_index: u64,
    chunk_ratio: u64,
) -> uefi::Result<vhdx::VhdxBatEntry> {
    let bat_index =
        vhdx::payload_bat_index(payload_index, chunk_ratio).ok_or(uefi::Status::LOAD_ERROR)?;
    read_bat_entry_at(bat, bat_index)
}

fn read_sector_bitmap_bat_entry(
    bat: &[u8],
    payload_index: u64,
    chunk_ratio: u64,
) -> uefi::Result<vhdx::VhdxBatEntry> {
    let bat_index = vhdx::sector_bitmap_bat_index(payload_index, chunk_ratio)
        .ok_or(uefi::Status::LOAD_ERROR)?;
    read_bat_entry_at(bat, bat_index)
}

fn read_bat_entry_at(bat: &[u8], bat_index: u64) -> uefi::Result<vhdx::VhdxBatEntry> {
    let bat_offset = usize::try_from(bat_index.checked_mul(8).ok_or(uefi::Status::LOAD_ERROR)?)
        .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
    let raw_entry = vhdx::read_le_u64(bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
    Ok(vhdx::parse_bat_entry(raw_entry))
}

fn bitmap_range_all_present(bitmap: &[u8], first_sector: u64, sector_count: u64) -> bool {
    (0..sector_count).all(|index| {
        let bit = first_sector + index;
        let byte_index = usize::try_from(bit / 8).ok();
        let mask = 1u8 << (bit % 8);
        byte_index
            .and_then(|offset| bitmap.get(offset))
            .is_some_and(|byte| byte & mask != 0)
    })
}
