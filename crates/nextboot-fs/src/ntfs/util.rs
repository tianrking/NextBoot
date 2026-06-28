use super::{FsError, NTFS_OEM_ID};
use crate::FileExtent;
use alloc::vec::Vec;

pub(super) fn apply_update_sequence(data: &mut [u8], sector_size: usize) -> Result<(), FsError> {
    if sector_size == 0 || data.len() < sector_size {
        return Err(FsError::Corrupted);
    }

    let usa_offset = read_u16(data, 4)? as usize;
    let usa_count = read_u16(data, 6)? as usize;
    if usa_count == 0
        || usa_offset
            .checked_add(usa_count.checked_mul(2).ok_or(FsError::Corrupted)?)
            .map_or(true, |end| end > data.len())
    {
        return Err(FsError::Corrupted);
    }

    let sequence = read_u16(data, usa_offset)?;
    let sector_count = data.len() / sector_size;
    if usa_count != sector_count + 1 {
        return Err(FsError::Corrupted);
    }

    for sector in 0..sector_count {
        let tail = (sector + 1)
            .checked_mul(sector_size)
            .and_then(|value| value.checked_sub(2))
            .ok_or(FsError::Corrupted)?;
        if read_u16(data, tail)? != sequence {
            return Err(FsError::Corrupted);
        }
        let replacement = read_u16(data, usa_offset + 2 * (sector + 1))?;
        data[tail..tail + 2].copy_from_slice(&replacement.to_le_bytes());
    }

    Ok(())
}

pub(super) fn decode_ntfs_size(encoded: i8, cluster_size: u64) -> Result<u32, FsError> {
    if encoded > 0 {
        let size = cluster_size
            .checked_mul(encoded as u64)
            .ok_or(FsError::Corrupted)?;
        u32::try_from(size).map_err(|_| FsError::Corrupted)
    } else if encoded < 0 {
        let shift = encoded.unsigned_abs();
        if shift >= 32 {
            return Err(FsError::Corrupted);
        }
        Ok(1u32 << shift)
    } else {
        Err(FsError::Corrupted)
    }
}

pub(super) fn push_extent(
    extents: &mut Vec<FileExtent>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    if block_count == 0 {
        return;
    }

    if let Some(last) = extents.last_mut() {
        if last.virtual_block_end() == virtual_block_start
            && last.physical_lba_end() == physical_lba
        {
            last.block_count += block_count;
            return;
        }
    }

    extents.push(FileExtent::new(
        virtual_block_start,
        physical_lba,
        block_count,
    ));
}

pub(super) fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::Corrupted)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub(super) fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::Corrupted)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn read_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 8).ok_or(FsError::Corrupted)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

pub(super) fn read_u48(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 6).ok_or(FsError::Corrupted)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0, 0,
    ]))
}

pub(super) fn read_file_reference(data: &[u8], offset: usize) -> Result<u64, FsError> {
    read_u48(data, offset)
}

pub(super) fn read_le_uint(data: &[u8]) -> u64 {
    let mut value = 0u64;
    for (index, byte) in data.iter().enumerate() {
        value |= u64::from(*byte) << (index * 8);
    }
    value
}

pub(super) fn read_le_int(data: &[u8]) -> i64 {
    if data.is_empty() {
        return 0;
    }

    let mut value = read_le_uint(data) as i64;
    let bits = data.len() * 8;
    if bits < 64 && data[data.len() - 1] & 0x80 != 0 {
        value |= (!0i64) << bits;
    }
    value
}

pub(super) fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }
    value.checked_add(divisor - 1).map(|value| value / divisor)
}

/// Check whether a sector buffer looks like an NTFS boot sector.
pub fn is_ntfs(data: &[u8]) -> bool {
    data.len() >= 512 && &data[3..11] == NTFS_OEM_ID && data[510] == 0x55 && data[511] == 0xAA
}
