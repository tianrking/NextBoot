use super::{
    bat_entry_count, parse_bat_entry, payload_bat_index, read_le_u64, VhdxMetadata,
    VHDX_BAT_STATE_FULLY_PRESENT, VHDX_BAT_STATE_NOT_PRESENT, VHDX_BAT_STATE_PARTIALLY_PRESENT,
    VHDX_BAT_STATE_UNDEFINED, VHDX_BAT_STATE_UNMAPPED, VHDX_BAT_STATE_ZERO,
};
use crate::{ImagePlanError, ImageSpan, ImageSpanSource};
use alloc::vec::Vec;

pub fn plan_vhdx_spans<F>(
    metadata: &VhdxMetadata,
    bat: &[u8],
    mut partial_block_is_self_contained: F,
) -> Result<Vec<ImageSpan>, ImagePlanError>
where
    F: FnMut(u64) -> Result<bool, ImagePlanError>,
{
    let block_size = u64::from(metadata.block_size);
    let payload_blocks =
        super::payload_block_count(metadata.virtual_disk_size, metadata.block_size)
            .ok_or(ImagePlanError::Invalid)?;
    let chunk_ratio = metadata.chunk_ratio().ok_or(ImagePlanError::Invalid)?;
    let bat_entries =
        bat_entry_count(payload_blocks, chunk_ratio).ok_or(ImagePlanError::Invalid)?;
    let bat_bytes = bat_entries.checked_mul(8).ok_or(ImagePlanError::Invalid)?;
    if u64::try_from(bat.len()).map_err(|_| ImagePlanError::Invalid)? < bat_bytes {
        return Err(ImagePlanError::Invalid);
    }

    let mut spans = Vec::new();
    spans
        .try_reserve_exact(usize::try_from(payload_blocks).map_err(|_| ImagePlanError::Invalid)?)
        .map_err(|_| ImagePlanError::Invalid)?;

    for payload_index in 0..payload_blocks {
        let entry = read_payload_entry(bat, payload_index, chunk_ratio)?;
        let virtual_offset = payload_index
            .checked_mul(block_size)
            .ok_or(ImagePlanError::Invalid)?;
        let byte_count = block_size.min(metadata.virtual_disk_size - virtual_offset);
        let source = match entry.state {
            VHDX_BAT_STATE_FULLY_PRESENT => ImageSpanSource::Image {
                file_offset: entry.file_offset,
            },
            VHDX_BAT_STATE_PARTIALLY_PRESENT => {
                if partial_block_is_self_contained(payload_index)? {
                    ImageSpanSource::Image {
                        file_offset: entry.file_offset,
                    }
                } else if metadata.has_parent {
                    ImageSpanSource::Parent
                } else {
                    return Err(ImagePlanError::Unsupported);
                }
            }
            VHDX_BAT_STATE_ZERO => ImageSpanSource::Zero,
            VHDX_BAT_STATE_NOT_PRESENT | VHDX_BAT_STATE_UNMAPPED => {
                if metadata.has_parent {
                    ImageSpanSource::Parent
                } else {
                    ImageSpanSource::Zero
                }
            }
            VHDX_BAT_STATE_UNDEFINED => return Err(ImagePlanError::Unsupported),
            _ => return Err(ImagePlanError::Invalid),
        };
        push_span(&mut spans, virtual_offset, byte_count, source)?;
    }

    Ok(spans)
}

fn read_payload_entry(
    bat: &[u8],
    payload_index: u64,
    chunk_ratio: u64,
) -> Result<super::VhdxBatEntry, ImagePlanError> {
    let bat_index = payload_bat_index(payload_index, chunk_ratio).ok_or(ImagePlanError::Invalid)?;
    let bat_offset = usize::try_from(bat_index.checked_mul(8).ok_or(ImagePlanError::Invalid)?)
        .map_err(|_| ImagePlanError::Invalid)?;
    let raw_entry = read_le_u64(bat, bat_offset).ok_or(ImagePlanError::Invalid)?;
    Ok(parse_bat_entry(raw_entry))
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
