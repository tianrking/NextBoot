use super::{
    decompress_xpress, lzx, read_le_u32, read_le_u64, WimCompression, WimMetadata, WimReadError,
    WimResourceHeader, XpressDecodeError, WIM_MAX_U32_RESOURCE_SIZE,
};
use alloc::vec::Vec;

pub fn read_resource_range(
    metadata: &WimMetadata,
    wim_file: &[u8],
    resource: &WimResourceHeader,
    offset: u64,
    out: &mut [u8],
) -> Result<(), WimReadError> {
    read_resource_range_with(
        metadata,
        wim_file.len() as u64,
        resource,
        offset,
        out,
        |offset, buf| {
            let start = usize::try_from(offset).map_err(|_| WimReadError::ResourceOutOfBounds)?;
            let end = start
                .checked_add(buf.len())
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            let source = wim_file
                .get(start..end)
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            buf.copy_from_slice(source);
            Ok(())
        },
    )
}

pub fn read_resource_range_with<F>(
    metadata: &WimMetadata,
    wim_len: u64,
    resource: &WimResourceHeader,
    offset: u64,
    out: &mut [u8],
    mut read_at: F,
) -> Result<(), WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    if out.is_empty() {
        return Ok(());
    }
    validate_resource_bounds(wim_len, resource)?;

    let read_end = offset
        .checked_add(out.len() as u64)
        .ok_or(WimReadError::InvalidRange)?;
    if read_end > resource.uncompressed_size {
        return Err(WimReadError::InvalidRange);
    }

    if !resource.is_compressed() && !resource.uses_packed_streams() {
        let start = resource
            .offset
            .checked_add(offset)
            .ok_or(WimReadError::ResourceOutOfBounds)?;
        read_at(start, out)?;
        return Ok(());
    }

    let mut remaining = out.len();
    let mut resource_offset = offset;
    let mut output_offset = 0usize;

    while remaining > 0 {
        let chunk = resource_chunk_span_with(metadata, resource, resource_offset, &mut read_at)?;
        let skip = resource_offset
            .checked_sub(chunk.uncompressed_offset)
            .ok_or(WimReadError::InvalidRange)?;
        let available = chunk
            .uncompressed_size
            .checked_sub(skip)
            .ok_or(WimReadError::InvalidRange)?;
        let copy_len = core::cmp::min(remaining as u64, available);
        let copy_len_usize =
            usize::try_from(copy_len).map_err(|_| WimReadError::ResourceOutOfBounds)?;
        if chunk.is_stored() {
            let source_start = chunk
                .compressed_offset
                .checked_add(skip)
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            read_at(
                source_start,
                &mut out[output_offset..output_offset + copy_len_usize],
            )?;
        } else if metadata.compression == WimCompression::Xpress {
            let decompressed = decompress_xpress_chunk_with(&chunk, &mut read_at)?;
            let skip = usize::try_from(skip).map_err(|_| WimReadError::ResourceOutOfBounds)?;
            let end = skip
                .checked_add(copy_len_usize)
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            out[output_offset..output_offset + copy_len_usize]
                .copy_from_slice(&decompressed[skip..end]);
        } else if metadata.compression == WimCompression::Lzx {
            let decompressed = decompress_lzx_chunk_with(&chunk, &mut read_at)?;
            let skip = usize::try_from(skip).map_err(|_| WimReadError::ResourceOutOfBounds)?;
            let end = skip
                .checked_add(copy_len_usize)
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            out[output_offset..output_offset + copy_len_usize]
                .copy_from_slice(&decompressed[skip..end]);
        } else {
            return Err(WimReadError::UnsupportedCompressedChunk {
                chunk_index: chunk.index,
                compressed_size: chunk.compressed_size,
                uncompressed_size: chunk.uncompressed_size,
            });
        }

        remaining -= copy_len_usize;
        output_offset += copy_len_usize;
        resource_offset = resource_offset
            .checked_add(copy_len)
            .ok_or(WimReadError::InvalidRange)?;
    }

    Ok(())
}

struct WimChunkSpan {
    index: u64,
    uncompressed_offset: u64,
    uncompressed_size: u64,
    compressed_offset: u64,
    compressed_size: u64,
}

impl WimChunkSpan {
    fn is_stored(&self) -> bool {
        self.compressed_size == self.uncompressed_size
    }
}

fn validate_resource_bounds(
    wim_len: u64,
    resource: &WimResourceHeader,
) -> Result<(), WimReadError> {
    let end = resource
        .offset
        .checked_add(resource.compressed_size)
        .ok_or(WimReadError::ResourceOutOfBounds)?;
    if end > wim_len {
        return Err(WimReadError::ResourceOutOfBounds);
    }
    Ok(())
}

fn resource_chunk_span_with<F>(
    metadata: &WimMetadata,
    resource: &WimResourceHeader,
    uncompressed_offset: u64,
    read_at: &mut F,
) -> Result<WimChunkSpan, WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    let chunk_len = u64::from(metadata.chunk_len);
    if chunk_len == 0 {
        return Err(WimReadError::InvalidChunkLength);
    }
    if uncompressed_offset >= resource.uncompressed_size {
        return Err(WimReadError::InvalidRange);
    }

    let chunk_index = uncompressed_offset / chunk_len;
    let chunk_uncompressed_offset = chunk_index
        .checked_mul(chunk_len)
        .ok_or(WimReadError::InvalidRange)?;
    let chunk_uncompressed_size = core::cmp::min(
        chunk_len,
        resource
            .uncompressed_size
            .checked_sub(chunk_uncompressed_offset)
            .ok_or(WimReadError::InvalidRange)?,
    );
    let chunk_start = resource_chunk_data_offset_with(resource, chunk_len, chunk_index, read_at)?;
    let chunk_end = resource_chunk_data_offset_with(resource, chunk_len, chunk_index + 1, read_at)?;
    if chunk_end < chunk_start || chunk_end > resource.compressed_size {
        return Err(WimReadError::InvalidChunkTable);
    }

    Ok(WimChunkSpan {
        index: chunk_index,
        uncompressed_offset: chunk_uncompressed_offset,
        uncompressed_size: chunk_uncompressed_size,
        compressed_offset: resource
            .offset
            .checked_add(chunk_start)
            .ok_or(WimReadError::ResourceOutOfBounds)?,
        compressed_size: chunk_end - chunk_start,
    })
}

fn read_compressed_chunk<F>(chunk: &WimChunkSpan, read_at: &mut F) -> Result<Vec<u8>, WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    let compressed_size =
        usize::try_from(chunk.compressed_size).map_err(|_| WimReadError::ResourceOutOfBounds)?;
    let mut compressed = Vec::new();
    compressed
        .try_reserve_exact(compressed_size)
        .map_err(|_| WimReadError::OutputReserveFailed)?;
    compressed.resize(compressed_size, 0);
    read_at(chunk.compressed_offset, &mut compressed)?;
    Ok(compressed)
}

fn decompress_xpress_chunk_with<F>(
    chunk: &WimChunkSpan,
    read_at: &mut F,
) -> Result<Vec<u8>, WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    let compressed = read_compressed_chunk(chunk, read_at)?;
    let mut out = Vec::new();
    let expected =
        usize::try_from(chunk.uncompressed_size).map_err(|_| WimReadError::ResourceOutOfBounds)?;
    out.try_reserve_exact(expected)
        .map_err(|_| WimReadError::OutputReserveFailed)?;
    out.resize(expected, 0);
    let produced =
        decompress_xpress(&compressed, &mut out).map_err(WimReadError::XpressDecodeFailed)?;
    if produced != expected {
        return Err(WimReadError::XpressDecodeFailed(
            XpressDecodeError::OutputOverflow,
        ));
    }
    Ok(out)
}

fn decompress_lzx_chunk_with<F>(
    chunk: &WimChunkSpan,
    read_at: &mut F,
) -> Result<Vec<u8>, WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    let compressed = read_compressed_chunk(chunk, read_at)?;
    let mut out = Vec::new();
    let expected =
        usize::try_from(chunk.uncompressed_size).map_err(|_| WimReadError::ResourceOutOfBounds)?;
    out.try_reserve_exact(expected)
        .map_err(|_| WimReadError::OutputReserveFailed)?;
    out.resize(expected, 0);
    let produced =
        lzx::decompress_lzx(&compressed, &mut out).map_err(WimReadError::LzxDecodeFailed)?;
    if produced != expected {
        return Err(WimReadError::LzxDecodeFailed(
            lzx::LzxDecodeError::OutputOverflow,
        ));
    }
    Ok(out)
}

fn resource_chunk_data_offset_with<F>(
    resource: &WimResourceHeader,
    chunk_len: u64,
    chunk_index: u64,
    read_at: &mut F,
) -> Result<u64, WimReadError>
where
    F: FnMut(u64, &mut [u8]) -> Result<(), WimReadError>,
{
    if resource.uncompressed_size == 0 {
        return Ok(0);
    }
    if chunk_len == 0 {
        return Err(WimReadError::InvalidChunkLength);
    }

    let offset_entry_len = if resource.uncompressed_size > WIM_MAX_U32_RESOURCE_SIZE {
        8
    } else {
        4
    };
    let chunk_count = div_round_up(resource.uncompressed_size, chunk_len)
        .ok_or(WimReadError::InvalidChunkTable)?;
    let chunks_len = chunk_count
        .saturating_sub(1)
        .checked_mul(offset_entry_len)
        .ok_or(WimReadError::InvalidChunkTable)?;
    if chunks_len > resource.compressed_size {
        return Err(WimReadError::InvalidChunkTable);
    }

    if chunk_index == 0 {
        return Ok(chunks_len);
    }
    if chunk_index >= chunk_count {
        return Ok(resource.compressed_size);
    }

    let table_entry_offset = resource
        .offset
        .checked_add(
            chunk_index
                .checked_sub(1)
                .ok_or(WimReadError::InvalidChunkTable)?
                .checked_mul(offset_entry_len)
                .ok_or(WimReadError::InvalidChunkTable)?,
        )
        .ok_or(WimReadError::ResourceOutOfBounds)?;
    let mut raw = [0u8; 8];
    let raw_len = usize::try_from(offset_entry_len).map_err(|_| WimReadError::InvalidChunkTable)?;
    read_at(table_entry_offset, &mut raw[..raw_len])?;
    let raw_offset = match offset_entry_len {
        4 => u64::from(read_le_u32(&raw, 0).ok_or(WimReadError::InvalidChunkTable)?),
        8 => read_le_u64(&raw, 0).ok_or(WimReadError::InvalidChunkTable)?,
        _ => return Err(WimReadError::InvalidChunkTable),
    };
    let offset = chunks_len
        .checked_add(raw_offset)
        .ok_or(WimReadError::InvalidChunkTable)?;
    if offset > resource.compressed_size {
        return Err(WimReadError::InvalidChunkTable);
    }

    Ok(offset)
}

fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }
    value
        .checked_add(divisor.checked_sub(1)?)?
        .checked_div(divisor)
}
