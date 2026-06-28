//! VDI metadata helpers.
//!
//! This module parses the VirtualBox/QEMU VDI 1.1 header and block map needed
//! to expose dynamic or static VDI images as a read-only UEFI Block IO device.

mod planner;
pub use planner::*;

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
const VDI_CREATE_UUID_OFFSET: usize = 0x188;
const VDI_MODIFY_UUID_OFFSET: usize = 0x198;
const VDI_LINKAGE_UUID_OFFSET: usize = 0x1a8;
const VDI_PARENT_MODIFY_UUID_OFFSET: usize = 0x1b8;

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
    pub create_uuid: [u8; 16],
    pub modify_uuid: [u8; 16],
    pub linkage_uuid: [u8; 16],
    pub parent_modify_uuid: [u8; 16],
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
    let create_uuid = read_uuid(header, VDI_CREATE_UUID_OFFSET)?;
    let modify_uuid = read_uuid(header, VDI_MODIFY_UUID_OFFSET)?;
    let linkage_uuid = read_uuid(header, VDI_LINKAGE_UUID_OFFSET)?;
    let parent_modify_uuid = read_uuid(header, VDI_PARENT_MODIFY_UUID_OFFSET)?;

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
        create_uuid,
        modify_uuid,
        linkage_uuid,
        parent_modify_uuid,
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

fn read_uuid(data: &[u8], offset: usize) -> Option<[u8; 16]> {
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(data.get(offset..offset.checked_add(16)?)?);
    Some(uuid)
}

#[cfg(test)]
mod tests;
