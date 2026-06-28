use super::{is_allocated_block, read_block_map_entry, VdiMetadata};
use crate::{ImagePlanError, ImageSpan, ImageSpanSource};
use alloc::vec::Vec;

pub fn plan_vdi_spans(
    metadata: &VdiMetadata,
    block_map: &[u8],
) -> Result<Vec<ImageSpan>, ImagePlanError> {
    let map_bytes = super::block_map_bytes(metadata.block_count).ok_or(ImagePlanError::Invalid)?;
    if u64::try_from(block_map.len()).map_err(|_| ImagePlanError::Invalid)? < map_bytes {
        return Err(ImagePlanError::Invalid);
    }

    let block_size = u64::from(metadata.block_size);
    let mut spans = Vec::new();
    spans
        .try_reserve_exact(
            usize::try_from(metadata.block_count).map_err(|_| ImagePlanError::Invalid)?,
        )
        .map_err(|_| ImagePlanError::Invalid)?;

    for block_index in 0..metadata.block_count {
        let virtual_offset = u64::from(block_index)
            .checked_mul(block_size)
            .ok_or(ImagePlanError::Invalid)?;
        if virtual_offset >= metadata.virtual_disk_size {
            break;
        }

        let byte_count = block_size.min(metadata.virtual_disk_size - virtual_offset);
        let map_entry =
            read_block_map_entry(block_map, block_index).ok_or(ImagePlanError::Invalid)?;
        let source = if is_allocated_block(map_entry) {
            if map_entry >= metadata.blocks_allocated {
                return Err(ImagePlanError::Invalid);
            }
            let file_offset = metadata
                .offset_data
                .checked_add(
                    u64::from(map_entry)
                        .checked_mul(block_size)
                        .ok_or(ImagePlanError::Invalid)?,
                )
                .ok_or(ImagePlanError::Invalid)?;
            ImageSpanSource::Image { file_offset }
        } else if metadata.is_differencing() {
            ImageSpanSource::Parent
        } else {
            ImageSpanSource::Zero
        };
        push_span(&mut spans, virtual_offset, byte_count, source)?;
    }

    Ok(spans)
}

fn push_span(
    spans: &mut Vec<ImageSpan>,
    virtual_offset: u64,
    byte_count: u64,
    source: ImageSpanSource,
) -> Result<(), ImagePlanError> {
    if byte_count == 0 {
        return Ok(());
    }
    if let Some(last) = spans.last_mut() {
        let last_end = last
            .virtual_offset
            .checked_add(last.byte_count)
            .ok_or(ImagePlanError::Invalid)?;
        if last_end == virtual_offset && same_merge_source(last.source, source) {
            last.byte_count = last
                .byte_count
                .checked_add(byte_count)
                .ok_or(ImagePlanError::Invalid)?;
            return Ok(());
        }
    }
    spans.push(ImageSpan {
        virtual_offset,
        byte_count,
        source,
    });
    Ok(())
}

fn same_merge_source(left: ImageSpanSource, right: ImageSpanSource) -> bool {
    matches!(
        (left, right),
        (ImageSpanSource::Parent, ImageSpanSource::Parent)
            | (ImageSpanSource::Zero, ImageSpanSource::Zero)
    )
}
