use super::virtio_error_to_uefi_status;
use nextboot_virtio::VirtualBlockIo;

pub(super) const SECTOR_SIZE: u64 = 512;
pub(super) const FOOTER_SIZE: usize = 512;
pub(super) const DYNAMIC_HEADER_SIZE: usize = 1024;
pub(super) const UNUSED_BAT_ENTRY: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, Copy)]
pub(super) struct DynamicFooter {
    pub(super) data_offset: u64,
    pub(super) virtual_size: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DynamicHeader {
    pub(super) table_offset: u64,
    pub(super) header_version: u32,
    pub(super) max_table_entries: u32,
    pub(super) block_size: u32,
}

pub(super) fn read_file_bytes(
    vbio: &VirtualBlockIo,
    offset: u64,
    buf: &mut [u8],
) -> uefi::Result<()> {
    vbio.read_bytes(vbio.media_id(), offset, buf)
        .map_err(virtio_error_to_uefi_status)?;
    Ok(())
}

pub(super) fn parse_dynamic_footer(data: &[u8]) -> Option<DynamicFooter> {
    if data.len() < FOOTER_SIZE || data.get(0..8)? != b"conectix" {
        return None;
    }

    let data_offset = read_be_u64(data, 16)?;
    let virtual_size = read_be_u64(data, 48)?;
    let disk_type = read_be_u32(data, 60)?;
    if data_offset == u64::MAX || disk_type != 3 {
        return None;
    }

    Some(DynamicFooter {
        data_offset,
        virtual_size,
    })
}

pub(super) fn parse_dynamic_header(data: &[u8]) -> Option<DynamicHeader> {
    if data.len() < DYNAMIC_HEADER_SIZE || data.get(0..8)? != b"cxsparse" {
        return None;
    }

    let table_offset = read_be_u64(data, 16)?;
    let header_version = read_be_u32(data, 24)?;
    let max_table_entries = read_be_u32(data, 28)?;
    let block_size = read_be_u32(data, 32)?;

    if table_offset == u64::MAX || max_table_entries == 0 || block_size == 0 {
        return None;
    }

    Some(DynamicHeader {
        table_offset,
        header_version,
        max_table_entries,
        block_size,
    })
}

pub(super) fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_be_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
