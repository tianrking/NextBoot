use super::source_volume::SourceVolumeReader;
use super::util::{align_up_u64, div_round_up};
use super::{vhd, BootManager};
use crate::vdi;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::media::block::BlockIO;

impl BootManager<'_> {
    pub(super) fn build_dynamic_vhd_block_io(
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
                .is_none_or(|end| end > self.iso.size)
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

    pub(super) fn build_vdi_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let metadata = self.read_vdi_metadata(&file_vbio)?;
        let parent = self.open_vdi_parent_backing(source_block_io, &metadata)?;

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.sector_size;

        let block_map = self.read_vdi_block_map(&file_vbio, &metadata, self.iso.size)?;

        let block_size = u64::from(metadata.block_size);
        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut parent_blocks = 0u64;
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
                if metadata.is_differencing() {
                    let Some(parent) = parent.as_ref() else {
                        warn!(
                            "VDI block {} in {} requires an unsupported parent chain",
                            block_index, self.iso.path
                        );
                        return Err(uefi::Status::UNSUPPORTED.into());
                    };
                    self.map_parent_vdi_range(
                        &mut byte_mapping,
                        parent,
                        virtual_start,
                        byte_count,
                    )?;
                    parent_blocks += 1;
                    continue;
                }
                zero_blocks += 1;
                continue;
            }

            if map_entry >= metadata.blocks_allocated {
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
                .is_none_or(|end| end > self.iso.size)
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
            "Mapped VDI {}: virtual={} bytes, block={} bytes, sector={} bytes, allocated_blocks={}, parent_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.sector_size,
            allocated_blocks,
            parent_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    pub(super) fn read_vdi_metadata(
        &self,
        file_vbio: &VirtualBlockIo,
    ) -> uefi::Result<vdi::VdiMetadata> {
        let mut header = [0u8; vdi::VDI_HEADER_SIZE];
        vhd::read_file_bytes(file_vbio, 0, &mut header)?;
        vdi::parse_vdi_metadata(&header).ok_or(uefi::Status::LOAD_ERROR.into())
    }

    pub(super) fn read_vdi_block_map(
        &self,
        file_vbio: &VirtualBlockIo,
        metadata: &vdi::VdiMetadata,
        file_size: u64,
    ) -> uefi::Result<Vec<u8>> {
        let map_bytes =
            vdi::block_map_bytes(metadata.block_count).ok_or(uefi::Status::LOAD_ERROR)?;
        if metadata
            .offset_blocks
            .checked_add(map_bytes)
            .is_none_or(|end| end > file_size || end > metadata.offset_data)
        {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let map_len = usize::try_from(map_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut block_map = Vec::new();
        block_map
            .try_reserve_exact(map_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        block_map.resize(map_len, 0);
        vhd::read_file_bytes(file_vbio, metadata.offset_blocks, &mut block_map)?;
        Ok(block_map)
    }

    pub(super) fn build_image_file_block_io(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
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

    pub(super) fn map_image_file_range_to_physical(
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
