use super::source_volume::{SourceVolumeFileMetadata, SourceVolumeFileSystem};
use super::BootManager;
use crate::vdi;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_image::ImageSpanSource;
use nextboot_virtio::mapping::ByteMappingTable;
use uefi::proto::media::block::BlockIO;

const MAX_VDI_PARENT_DEPTH: u8 = 8;

pub(super) struct VdiParentBacking {
    path: String,
    file: SourceVolumeFileMetadata,
    metadata: vdi::VdiMetadata,
    block_map: Vec<u8>,
    parent: Option<Box<VdiParentBacking>>,
}

impl BootManager<'_> {
    pub(super) fn open_vdi_parent_backing(
        &self,
        source_block_io: &BlockIO,
        child_metadata: &vdi::VdiMetadata,
    ) -> uefi::Result<Option<VdiParentBacking>> {
        self.open_vdi_parent_backing_for_child(source_block_io, &self.iso.path, child_metadata, 0)
    }

    fn open_vdi_parent_backing_for_child(
        &self,
        source_block_io: &BlockIO,
        child_path: &str,
        child_metadata: &vdi::VdiMetadata,
        depth: u8,
    ) -> uefi::Result<Option<VdiParentBacking>> {
        if !child_metadata.is_differencing() {
            return Ok(None);
        }
        if depth >= MAX_VDI_PARENT_DEPTH {
            warn!(
                "VDI parent chain for {} exceeded {} levels",
                child_path, MAX_VDI_PARENT_DEPTH
            );
            return Ok(None);
        }

        let fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        let parent_dir = parent_dir(child_path);
        let entries = fs.read_dir(&parent_dir)?;
        let mut candidate_files = 0u32;
        for entry in entries {
            if entry.is_dir || !is_vdi_parent_candidate_name(&entry.name) {
                continue;
            }
            candidate_files += 1;

            let candidate_path = join_path(&parent_dir, &entry.name);
            if candidate_path.eq_ignore_ascii_case(child_path) {
                continue;
            }

            let Ok(parent_file) = fs.file_metadata(&candidate_path) else {
                continue;
            };
            let parent_vbio =
                self.build_source_volume_file_block_io(source_block_io, &parent_file)?;
            let Ok(parent_metadata) = self.read_vdi_metadata(&parent_vbio) else {
                continue;
            };
            if !vdi_parent_matches(child_metadata, &parent_metadata) {
                continue;
            }

            let block_map =
                self.read_vdi_block_map(&parent_vbio, &parent_metadata, parent_file.size)?;
            let parent = self
                .open_vdi_parent_backing_for_child(
                    source_block_io,
                    &candidate_path,
                    &parent_metadata,
                    depth + 1,
                )?
                .map(Box::new);
            info!("Resolved VDI parent for {}: {}", child_path, candidate_path);
            return Ok(Some(VdiParentBacking {
                path: candidate_path,
                file: parent_file,
                metadata: parent_metadata,
                block_map,
                parent,
            }));
        }

        warn!(
            "VDI parent for {} was not found in {} from {} .vdi/.vdibase candidate(s); copy the parent VDI into the same directory with matching linkage UUIDs",
            child_path,
            parent_dir,
            candidate_files
        );
        Ok(None)
    }

    pub(super) fn map_parent_vdi_range(
        &self,
        byte_mapping: &mut ByteMappingTable,
        parent: &VdiParentBacking,
        virtual_start: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let parent_spans = vdi::plan_vdi_spans(&parent.metadata, &parent.block_map)
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
                    let Some(next_parent) = parent.parent.as_ref() else {
                        warn!(
                            "VDI parent {} still requires a missing higher parent chain",
                            parent.path
                        );
                        return Err(uefi::Status::UNSUPPORTED.into());
                    };
                    self.map_parent_vdi_range(
                        byte_mapping,
                        next_parent,
                        overlap_start,
                        overlap_len,
                    )?;
                }
            }
        }

        Ok(())
    }
}

fn vdi_parent_matches(child: &vdi::VdiMetadata, parent: &vdi::VdiMetadata) -> bool {
    child.virtual_disk_size == parent.virtual_disk_size
        && child.block_size == parent.block_size
        && child.sector_size == parent.sector_size
        && child.linkage_uuid == parent.create_uuid
        && child.parent_modify_uuid == parent.modify_uuid
}

fn parent_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(index) => trimmed[..index].to_string(),
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" || parent.is_empty() {
        alloc::format!("/{}", name)
    } else {
        alloc::format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn is_vdi_parent_candidate_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".vdi") || lower.ends_with(".vdibase")
}
