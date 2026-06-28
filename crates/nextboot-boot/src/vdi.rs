//! VDI metadata helpers.
//!
//! This module parses the VirtualBox/QEMU VDI 1.1 header and block map needed
//! to expose dynamic or static VDI images as a read-only UEFI Block IO device.

pub const VDI_HEADER_SIZE: usize = 512;
pub const VDI_SIGNATURE: u32 = 0xbeda_107f;
pub const VDI_VERSION_1_1: u32 = 0x0001_0001;
pub const VDI_TYPE_DYNAMIC: u32 = 1;
pub const VDI_TYPE_STATIC: u32 = 2;
pub const VDI_TYPE_DIFFERENCING: u32 = 4;
pub const VDI_UNALLOCATED: u32 = 0xffff_ffff;
pub const VDI_DISCARDED: u32 = 0xffff_fffe;
pub const VDI_DEFAULT_SECTOR_SIZE: u32 = 512;

const VDI_SIGNATURE_OFFSET: usize = 0x40;
const VDI_VERSION_OFFSET: usize = 0x44;
const VDI_HEADER_SIZE_OFFSET: usize = 0x48;
const VDI_IMAGE_TYPE_OFFSET: usize = 0x4c;
const VDI_OFFSET_BLOCKS_OFFSET: usize = 0x154;
const VDI_OFFSET_DATA_OFFSET: usize = 0x158;
const VDI_SECTOR_SIZE_OFFSET: usize = 0x168;
const VDI_DISK_SIZE_OFFSET: usize = 0x170;
const VDI_BLOCK_SIZE_OFFSET: usize = 0x178;
const VDI_BLOCK_EXTRA_OFFSET: usize = 0x17c;
const VDI_BLOCK_COUNT_OFFSET: usize = 0x180;
const VDI_BLOCKS_ALLOCATED_OFFSET: usize = 0x184;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VdiMetadata {
    pub image_type: u32,
    pub virtual_disk_size: u64,
    pub block_size: u32,
    pub block_extra_size: u32,
    pub block_count: u32,
    pub blocks_allocated: u32,
    pub offset_blocks: u64,
    pub offset_data: u64,
    pub sector_size: u32,
}

impl VdiMetadata {
    pub fn is_dynamic(&self) -> bool {
        self.image_type == VDI_TYPE_DYNAMIC
    }

    pub fn is_static(&self) -> bool {
        self.image_type == VDI_TYPE_STATIC
    }

    pub fn is_differencing(&self) -> bool {
        self.image_type == VDI_TYPE_DIFFERENCING
    }
}

pub fn parse_vdi_metadata(header: &[u8]) -> Option<VdiMetadata> {
    if header.len() < VDI_HEADER_SIZE {
        return None;
    }

    if read_le_u32(header, VDI_SIGNATURE_OFFSET)? != VDI_SIGNATURE {
        return None;
    }
    if read_le_u32(header, VDI_VERSION_OFFSET)? != VDI_VERSION_1_1 {
        return None;
    }

    let header_size = usize::try_from(read_le_u32(header, VDI_HEADER_SIZE_OFFSET)?).ok()?;
    if header_size == 0 || VDI_HEADER_SIZE_OFFSET.checked_add(header_size)? > header.len() {
        return None;
    }

    let image_type = read_le_u32(header, VDI_IMAGE_TYPE_OFFSET)?;
    if !matches!(
        image_type,
        VDI_TYPE_DYNAMIC | VDI_TYPE_STATIC | VDI_TYPE_DIFFERENCING
    ) {
        return None;
    }

    let offset_blocks = u64::from(read_le_u32(header, VDI_OFFSET_BLOCKS_OFFSET)?);
    let offset_data = u64::from(read_le_u32(header, VDI_OFFSET_DATA_OFFSET)?);
    let sector_size = read_le_u32(header, VDI_SECTOR_SIZE_OFFSET)?;
    let virtual_disk_size = read_le_u64(header, VDI_DISK_SIZE_OFFSET)?;
    let block_size = read_le_u32(header, VDI_BLOCK_SIZE_OFFSET)?;
    let block_extra_size = read_le_u32(header, VDI_BLOCK_EXTRA_OFFSET)?;
    let block_count = read_le_u32(header, VDI_BLOCK_COUNT_OFFSET)?;
    let blocks_allocated = read_le_u32(header, VDI_BLOCKS_ALLOCATED_OFFSET)?;

    if virtual_disk_size == 0
        || block_size == 0
        || block_count == 0
        || !block_size.is_power_of_two()
        || sector_size != VDI_DEFAULT_SECTOR_SIZE
        || offset_blocks % u64::from(VDI_DEFAULT_SECTOR_SIZE) != 0
        || offset_data % u64::from(VDI_DEFAULT_SECTOR_SIZE) != 0
    {
        return None;
    }

    let block_size_u64 = u64::from(block_size);
    if virtual_disk_size > u64::from(block_count).checked_mul(block_size_u64)? {
        return None;
    }

    let map_bytes = block_map_bytes(block_count)?;
    if offset_blocks.checked_add(map_bytes)? > offset_data {
        return None;
    }

    Some(VdiMetadata {
        image_type,
        virtual_disk_size,
        block_size,
        block_extra_size,
        block_count,
        blocks_allocated,
        offset_blocks,
        offset_data,
        sector_size,
    })
}

pub fn block_map_bytes(block_count: u32) -> Option<u64> {
    u64::from(block_count).checked_mul(4)
}

pub fn read_block_map_entry(map: &[u8], index: u32) -> Option<u32> {
    let offset = usize::try_from(u64::from(index).checked_mul(4)?).ok()?;
    read_le_u32(map, offset)
}

pub fn is_allocated_block(entry: u32) -> bool {
    entry < VDI_DISCARDED
}

pub fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_le_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_dynamic_vdi_header() {
        let header = make_vdi_header(VDI_TYPE_DYNAMIC, 8 * 1024 * 1024, 1024 * 1024, 8, 1);

        let metadata = parse_vdi_metadata(&header).unwrap();
        assert!(metadata.is_dynamic());
        assert_eq!(metadata.virtual_disk_size, 8 * 1024 * 1024);
        assert_eq!(metadata.block_size, 1024 * 1024);
        assert_eq!(metadata.block_count, 8);
        assert_eq!(metadata.blocks_allocated, 1);
        assert_eq!(metadata.offset_blocks, 0x200);
        assert_eq!(metadata.offset_data, 0x400);
        assert_eq!(metadata.sector_size, 512);
    }

    #[test]
    fn parses_static_vdi_header() {
        let header = make_vdi_header(VDI_TYPE_STATIC, 4 * 1024 * 1024, 1024 * 1024, 4, 4);

        let metadata = parse_vdi_metadata(&header).unwrap();
        assert!(metadata.is_static());
        assert_eq!(metadata.virtual_disk_size, 4 * 1024 * 1024);
        assert_eq!(metadata.block_count, 4);
        assert_eq!(metadata.blocks_allocated, 4);
    }

    #[test]
    fn parses_differencing_vdi_header() {
        let header = make_vdi_header(VDI_TYPE_DIFFERENCING, 4 * 1024 * 1024, 1024 * 1024, 4, 2);

        let metadata = parse_vdi_metadata(&header).unwrap();
        assert!(metadata.is_differencing());
        assert_eq!(metadata.virtual_disk_size, 4 * 1024 * 1024);
        assert_eq!(metadata.blocks_allocated, 2);
    }

    #[test]
    fn rejects_bad_signature_and_version() {
        let mut header = make_vdi_header(VDI_TYPE_DYNAMIC, 1024 * 1024, 1024 * 1024, 1, 0);
        header[VDI_SIGNATURE_OFFSET] = 0;
        assert!(parse_vdi_metadata(&header).is_none());

        let mut header = make_vdi_header(VDI_TYPE_DYNAMIC, 1024 * 1024, 1024 * 1024, 1, 0);
        write_le_u32(&mut header, VDI_VERSION_OFFSET, 0x0001_0000);
        assert!(parse_vdi_metadata(&header).is_none());
    }

    #[test]
    fn block_map_entries_classify_sparse_and_allocated_blocks() {
        let mut map = [0u8; 12];
        write_le_u32(&mut map, 0, 0);
        write_le_u32(&mut map, 4, VDI_DISCARDED);
        write_le_u32(&mut map, 8, VDI_UNALLOCATED);

        assert_eq!(read_block_map_entry(&map, 0), Some(0));
        assert!(is_allocated_block(read_block_map_entry(&map, 0).unwrap()));
        assert!(!is_allocated_block(read_block_map_entry(&map, 1).unwrap()));
        assert!(!is_allocated_block(read_block_map_entry(&map, 2).unwrap()));
    }

    fn make_vdi_header(
        image_type: u32,
        virtual_disk_size: u64,
        block_size: u32,
        block_count: u32,
        blocks_allocated: u32,
    ) -> [u8; VDI_HEADER_SIZE] {
        let mut header = [0u8; VDI_HEADER_SIZE];
        let file_info = b"<<< QEMU VM Virtual Disk Image >>>\n";
        header[..file_info.len()].copy_from_slice(file_info);
        write_le_u32(&mut header, VDI_SIGNATURE_OFFSET, VDI_SIGNATURE);
        write_le_u32(&mut header, VDI_VERSION_OFFSET, VDI_VERSION_1_1);
        write_le_u32(&mut header, VDI_HEADER_SIZE_OFFSET, 0x180);
        write_le_u32(&mut header, VDI_IMAGE_TYPE_OFFSET, image_type);
        write_le_u32(&mut header, VDI_OFFSET_BLOCKS_OFFSET, 0x200);
        write_le_u32(&mut header, VDI_OFFSET_DATA_OFFSET, 0x400);
        write_le_u32(&mut header, VDI_SECTOR_SIZE_OFFSET, VDI_DEFAULT_SECTOR_SIZE);
        write_le_u64(&mut header, VDI_DISK_SIZE_OFFSET, virtual_disk_size);
        write_le_u32(&mut header, VDI_BLOCK_SIZE_OFFSET, block_size);
        write_le_u32(&mut header, VDI_BLOCK_COUNT_OFFSET, block_count);
        write_le_u32(&mut header, VDI_BLOCKS_ALLOCATED_OFFSET, blocks_allocated);
        header
    }

    fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
