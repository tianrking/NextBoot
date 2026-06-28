use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use uefi::proto::device_path::DevicePath;

pub(crate) fn device_path_to_vec(device_path: &DevicePath) -> Result<Vec<u8>, &'static str> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }.ok_or("invalid device path")?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Ok(bytes.to_vec())
}

unsafe fn device_path_byte_len(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }
    let mut offset = 0usize;
    loop {
        let node = unsafe { ptr.add(offset) };
        let node_type = unsafe { ptr::read_unaligned(node) };
        let node_subtype = unsafe { ptr::read_unaligned(node.add(1)) };
        let len_lo = unsafe { ptr::read_unaligned(node.add(2)) };
        let len_hi = unsafe { ptr::read_unaligned(node.add(3)) };
        let node_len = u16::from_le_bytes([len_lo, len_hi]) as usize;
        if node_len < 4 {
            return None;
        }
        offset = offset.checked_add(node_len)?;
        if node_type == 0x7f && node_subtype == 0xff {
            return Some(offset);
        }
    }
}

pub(crate) fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn read_le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

pub(crate) fn write_le_u32(
    bytes: &mut [u8],
    offset: usize,
    value: u32,
) -> Result<(), &'static str> {
    bytes
        .get_mut(offset..offset + 4)
        .ok_or("write is out of bounds")?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

pub(crate) fn write_le_u64(
    bytes: &mut [u8],
    offset: usize,
    value: u64,
) -> Result<(), &'static str> {
    bytes
        .get_mut(offset..offset + 8)
        .ok_or("write is out of bounds")?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

pub(crate) fn div_round_up(value: usize, divisor: usize) -> usize {
    if divisor == 0 {
        0
    } else {
        value.saturating_add(divisor - 1) / divisor
    }
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

pub(crate) fn update_exfat_boot_checksum(
    boot_region: &mut [u8],
    block_size: u32,
) -> Result<(), &'static str> {
    const CHECKSUM_SECTOR: usize = 11;

    let block_size = usize::try_from(block_size).map_err(|_| "invalid block size")?;
    let checksum_sector = CHECKSUM_SECTOR
        .checked_mul(block_size)
        .ok_or("exFAT checksum offset overflows")?;
    let checksum_end = checksum_sector
        .checked_add(block_size)
        .ok_or("exFAT checksum end overflows")?;
    if boot_region.len() < checksum_end {
        return Err("truncated exFAT boot region");
    }

    for byte in boot_region
        .get_mut(checksum_sector..checksum_end)
        .ok_or("truncated exFAT checksum sector")?
    {
        *byte = 0;
    }

    let mut checksum = 0u32;
    for (offset, byte) in boot_region
        .get(..checksum_sector)
        .ok_or("truncated exFAT boot checksum input")?
        .iter()
        .enumerate()
    {
        if matches!(offset, 106 | 107 | 112) {
            continue;
        }
        checksum = checksum.rotate_right(1).wrapping_add(u32::from(*byte));
    }

    for chunk in boot_region
        .get_mut(checksum_sector..checksum_end)
        .ok_or("truncated exFAT checksum sector")?
        .chunks_exact_mut(4)
    {
        chunk.copy_from_slice(&checksum.to_le_bytes());
    }
    Ok(())
}

pub(crate) fn zeroed_vec(len: usize) -> Vec<u8> {
    vec![0u8; len]
}
