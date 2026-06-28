use super::source_volume::{SourceVolumeFileMetadata, SourceVolumeFileSystem, SourceVolumeReader};
use super::{vhd, BootManager};
use crate::vhdx;
use alloc::string::String;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_image::ImageSpanSource;
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};
use uefi::proto::media::block::BlockIO;

pub(super) struct VhdxParentBacking {
    path: String,
    file: SourceVolumeFileMetadata,
    metadata: vhdx::VhdxMetadata,
    bat: Vec<u8>,
}

impl BootManager<'_> {
    pub(super) fn open_vhdx_parent_backing(
        &self,
        source_block_io: &BlockIO,
        child_metadata: &vhdx::VhdxMetadata,
    ) -> uefi::Result<Option<VhdxParentBacking>> {
        if !child_metadata.has_parent {
            return Ok(None);
        }

        let candidates = child_metadata.parent_paths();
        if candidates.is_empty() {
            warn!(
                "VHDX {} has parent flag but no same-volume parent candidates",
                self.iso.path
            );
            return Ok(None);
        }

        let fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        for locator_path in candidates {
            let Some(parent_path) =
                vhdx::resolve_same_volume_parent_path(&self.iso.path, locator_path)
            else {
                continue;
            };
            if parent_path.eq_ignore_ascii_case(&self.iso.path) {
                continue;
            }

            let Ok(parent_file) = fs.file_metadata(&parent_path) else {
                continue;
            };
            let parent_vbio =
                self.build_source_volume_file_block_io(source_block_io, &parent_file)?;
            let (regions, parent_metadata) = self.read_vhdx_layout(&parent_vbio)?;
            if parent_metadata.virtual_disk_size != child_metadata.virtual_disk_size
                || parent_metadata.logical_sector_size != child_metadata.logical_sector_size
            {
                warn!(
                    "VHDX parent {} does not match child {} geometry",
                    parent_path, self.iso.path
                );
                continue;
            }

            let payload_blocks = vhdx::payload_block_count(
                parent_metadata.virtual_disk_size,
                parent_metadata.block_size,
            )
            .ok_or(uefi::Status::LOAD_ERROR)?;
            let chunk_ratio = parent_metadata
                .chunk_ratio()
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let bat = self.read_vhdx_bat(&parent_vbio, &regions, payload_blocks, chunk_ratio)?;
            info!(
                "Resolved VHDX parent for {}: {}",
                self.iso.path, parent_path
            );
            return Ok(Some(VhdxParentBacking {
                path: parent_path,
                file: parent_file,
                metadata: parent_metadata,
                bat,
            }));
        }

        warn!(
            "VHDX parent for {} was not found from locator paths",
            self.iso.path
        );
        Ok(None)
    }

    pub(super) fn build_source_volume_file_block_io(
        &self,
        source_block_io: &BlockIO,
        file: &SourceVolumeFileMetadata,
    ) -> uefi::Result<VirtualBlockIo> {
        let start_lba = file
            .extents
            .first()
            .map(|extent| extent.physical_lba)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::HardDisk,
            start_lba,
            file.size,
            vhd::SECTOR_SIZE as u32,
        )
        .with_physical_block_size(file.block_size)
        .with_name(&file.path);
        let extents: Vec<(u64, u64, u64)> = file
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
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
        let reader = SourceVolumeReader::new(source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);
        Ok(vbio)
    }

    pub(super) fn map_parent_vhdx_range(
        &self,
        byte_mapping: &mut ByteMappingTable,
        parent: &VhdxParentBacking,
        virtual_start: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let parent_spans = vhdx::plan_vhdx_spans(&parent.metadata, &parent.bat, |_| Ok(false))
            .map_err(|_| uefi::Status::UNSUPPORTED)?;
        let virtual_end = virtual_start
            .checked_add(byte_count)
            .ok_or(uefi::Status::LOAD_ERROR)?;

        for span in parent_spans {
            let span_end = span
                .virtual_offset
                .checked_add(span.byte_count)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let overlap_start = virtual_start.max(span.virtual_offset);
            let overlap_end = virtual_end.min(span_end);
            if overlap_start >= overlap_end {
                continue;
            }

            let overlap_len = overlap_end - overlap_start;
            match span.source {
                ImageSpanSource::Image { file_offset } => {
                    let parent_file_offset = file_offset
                        .checked_add(overlap_start - span.virtual_offset)
                        .ok_or(uefi::Status::LOAD_ERROR)?;
                    self.map_source_file_range_to_physical(
                        byte_mapping,
                        &parent.file,
                        overlap_start,
                        parent_file_offset,
                        overlap_len,
                    )?;
                }
                ImageSpanSource::Zero => {}
                ImageSpanSource::Parent => {
                    warn!(
                        "VHDX parent {} still requires a higher parent chain",
                        parent.path
                    );
                    return Err(uefi::Status::UNSUPPORTED.into());
                }
            }
        }

        Ok(())
    }

    pub(super) fn map_source_file_range_to_physical(
        &self,
        table: &mut ByteMappingTable,
        file: &SourceVolumeFileMetadata,
        virtual_start: u64,
        file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let source_block_size = u64::from(file.block_size);
        if source_block_size == 0 {
            return Err(uefi::Status::INVALID_PARAMETER.into());
        }

        let file_end = file_offset
            .checked_add(byte_count)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let mut cursor = file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &file.extents {
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
                warn!(
                    "Could not map {} parent file byte offset {}",
                    file.path, cursor
                );
                return Err(uefi::Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }
}
