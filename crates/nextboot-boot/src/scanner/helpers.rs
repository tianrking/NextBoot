use super::model::{ImageFormat, IsoExtent};
use alloc::vec::Vec;
use nextboot_fs::BlockIoOps;

#[derive(Debug, Clone, Copy)]
pub(super) struct VhdFooterInfo {
    pub(super) image_format: ImageFormat,
    pub(super) virtual_size: u64,
}

pub(super) fn has_mbr_signature(block: &[u8]) -> bool {
    block.get(510) == Some(&0x55) && block.get(511) == Some(&0xaa)
}

pub(super) fn read_block_range(
    shared: &nextboot_fs::SharedBlockIo,
    start_lba: u64,
    byte_len: usize,
) -> Option<Vec<u8>> {
    if byte_len == 0 {
        return Some(Vec::new());
    }

    let block_size = usize::try_from(shared.block_size()).ok()?;
    if block_size == 0 {
        return None;
    }
    let block_count = div_round_up_usize(byte_len, block_size);
    let block_count_u64 = u64::try_from(block_count).ok()?;
    if start_lba
        .checked_add(block_count_u64)
        .map_or(true, |end| end > shared.total_blocks())
    {
        return None;
    }

    let len = block_count.checked_mul(block_size)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).ok()?;
    bytes.resize(len, 0);
    shared.read_blocks(start_lba, &mut bytes).ok()?;
    bytes.truncate(byte_len);
    Some(bytes)
}

pub(super) fn offset_extents_for_physical_read(
    extents: &[IsoExtent],
    lba_offset: u64,
) -> Option<Vec<IsoExtent>> {
    if lba_offset == 0 {
        return Some(extents.to_vec());
    }

    let mut out = Vec::new();
    out.try_reserve_exact(extents.len()).ok()?;
    for extent in extents {
        out.push(IsoExtent {
            virtual_block_start: extent.virtual_block_start,
            physical_lba: extent.physical_lba.checked_add(lba_offset)?,
            block_count: extent.block_count,
        });
    }
    Some(out)
}

pub(super) fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(super) fn read_le_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

pub(super) fn parse_vhd_footer(footer: &[u8]) -> Option<VhdFooterInfo> {
    if footer.len() < 512 || footer.get(0..8)? != b"conectix" {
        return None;
    }

    let virtual_size = u64::from_be_bytes(footer.get(48..56)?.try_into().ok()?);
    let disk_type = u32::from_be_bytes(footer.get(60..64)?.try_into().ok()?);
    if virtual_size == 0 {
        return None;
    }

    Some(VhdFooterInfo {
        image_format: ImageFormat::from_vhd_disk_type(disk_type),
        virtual_size,
    })
}

pub(super) fn default_virtual_block_size(image_format: ImageFormat) -> Option<u32> {
    if image_format.uses_512_byte_virtual_sectors() {
        Some(512)
    } else {
        None
    }
}

fn div_round_up_usize(value: usize, divisor: usize) -> usize {
    if divisor == 0 {
        0
    } else {
        value.saturating_add(divisor - 1) / divisor
    }
}
