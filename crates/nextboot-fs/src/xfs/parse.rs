use crate::FsError;

pub(super) fn read_be_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::Corrupted)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

pub(super) fn read_be_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::Corrupted)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn read_be_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 8).ok_or(FsError::Corrupted)?;
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

pub(super) fn read_log_or_ceil(data: &[u8], offset: usize, value: u32) -> u8 {
    data.get(offset)
        .copied()
        .filter(|log| *log != 0)
        .or_else(|| ceil_log2(value))
        .unwrap_or(0)
}

pub(super) fn nonzero_log2(value: u32) -> Option<u8> {
    if value == 0 || !value.is_power_of_two() {
        return None;
    }
    Some(value.trailing_zeros() as u8)
}

fn ceil_log2(value: u32) -> Option<u8> {
    if value == 0 {
        return None;
    }
    Some((u32::BITS - (value - 1).leading_zeros()) as u8)
}
