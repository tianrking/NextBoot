//! VHDX metadata helpers.
//!
//! This module parses only the read-only metadata needed to expose a clean
//! fixed or dynamic VHDX as a UEFI Block IO device. Differencing images still
//! need parent-chain support and are rejected by the boot path.

pub const VHDX_HEADER_SECTION_SIZE: usize = 1024 * 1024;
pub const VHDX_REGION_TABLE_SIZE: usize = 64 * 1024;
pub const VHDX_REGION_TABLE_1_OFFSET: usize = 192 * 1024;
pub const VHDX_REGION_TABLE_2_OFFSET: usize = 256 * 1024;
pub const VHDX_BAT_STATE_NOT_PRESENT: u8 = 0;
pub const VHDX_BAT_STATE_UNDEFINED: u8 = 1;
pub const VHDX_BAT_STATE_ZERO: u8 = 2;
pub const VHDX_BAT_STATE_UNMAPPED: u8 = 3;
pub const VHDX_BAT_STATE_FULLY_PRESENT: u8 = 6;
pub const VHDX_BAT_STATE_PARTIALLY_PRESENT: u8 = 7;
pub const VHDX_MIB: u64 = 1024 * 1024;

const VHDX_FILE_IDENTIFIER: &[u8; 8] = b"vhdxfile";
const VHDX_REGION_SIGNATURE: &[u8; 4] = b"regi";
const VHDX_METADATA_SIGNATURE: &[u8; 8] = b"metadata";
const VHDX_MAX_REGION_ENTRIES: u32 = 2047;
const VHDX_MAX_METADATA_ENTRIES: u16 = 2047;
const VHDX_MIN_PAYLOAD_BLOCK_SIZE: u32 = 1024 * 1024;
const VHDX_MAX_PAYLOAD_BLOCK_SIZE: u32 = 256 * 1024 * 1024;
const VHDX_FILE_PARAMETERS_HAS_PARENT: u32 = 1 << 1;

const BAT_REGION_GUID: [u8; 16] = [
    0x66, 0x77, 0xC2, 0x2D, 0x23, 0xF6, 0x00, 0x42, 0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08,
];
const METADATA_REGION_GUID: [u8; 16] = [
    0x06, 0xA2, 0x7C, 0x8B, 0x90, 0x47, 0x9A, 0x4B, 0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E,
];
const FILE_PARAMETERS_GUID: [u8; 16] = [
    0x37, 0x67, 0xA1, 0xCA, 0x36, 0xFA, 0x43, 0x4D, 0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B,
];
const VIRTUAL_DISK_SIZE_GUID: [u8; 16] = [
    0x24, 0x42, 0xA5, 0x2F, 0x1B, 0xCD, 0x76, 0x48, 0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8,
];
const LOGICAL_SECTOR_SIZE_GUID: [u8; 16] = [
    0x1D, 0xBF, 0x41, 0x81, 0x6F, 0xA9, 0x09, 0x47, 0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F,
];
const PHYSICAL_SECTOR_SIZE_GUID: [u8; 16] = [
    0xC7, 0x48, 0xA3, 0xCD, 0x5D, 0x44, 0x71, 0x44, 0x9C, 0xC9, 0xE9, 0x88, 0x52, 0x51, 0xC5, 0x56,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VhdxRegions {
    pub bat_offset: u64,
    pub bat_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VhdxMetadata {
    pub virtual_disk_size: u64,
    pub block_size: u32,
    pub logical_sector_size: u32,
    pub physical_sector_size: u32,
    pub has_parent: bool,
}

impl VhdxMetadata {
    pub fn chunk_ratio(&self) -> Option<u64> {
        let chunk_bytes = (1u64 << 23).checked_mul(u64::from(self.logical_sector_size))?;
        let block_size = u64::from(self.block_size);
        if block_size == 0 || chunk_bytes % block_size != 0 {
            return None;
        }

        let ratio = chunk_bytes / block_size;
        if ratio == 0 {
            None
        } else {
            Some(ratio)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VhdxBatEntry {
    pub state: u8,
    pub file_offset: u64,
}

pub fn parse_vhdx_regions(header_section: &[u8]) -> Option<VhdxRegions> {
    if header_section.get(0..8)? != VHDX_FILE_IDENTIFIER {
        return None;
    }

    parse_region_table_at(header_section, VHDX_REGION_TABLE_1_OFFSET)
        .or_else(|| parse_region_table_at(header_section, VHDX_REGION_TABLE_2_OFFSET))
}

pub fn parse_vhdx_metadata(metadata_region: &[u8]) -> Option<VhdxMetadata> {
    if metadata_region.len() < 32 || metadata_region.get(0..8)? != VHDX_METADATA_SIGNATURE {
        return None;
    }

    let entry_count = read_le_u16(metadata_region, 10)?;
    if entry_count > VHDX_MAX_METADATA_ENTRIES {
        return None;
    }

    let mut block_size = None;
    let mut file_parameters = None;
    let mut virtual_disk_size = None;
    let mut logical_sector_size = None;
    let mut physical_sector_size = None;

    for index in 0..usize::from(entry_count) {
        let entry_offset = 32usize.checked_add(index.checked_mul(32)?)?;
        let entry = metadata_region.get(entry_offset..entry_offset.checked_add(32)?)?;
        let item_id = entry.get(0..16)?;
        let item_offset = usize::try_from(read_le_u32(entry, 16)?).ok()?;
        let item_length = usize::try_from(read_le_u32(entry, 20)?).ok()?;
        let item_end = item_offset.checked_add(item_length)?;
        let item = metadata_region.get(item_offset..item_end)?;

        if item_id == FILE_PARAMETERS_GUID {
            if item.len() < 8 {
                return None;
            }
            block_size = Some(read_le_u32(item, 0)?);
            file_parameters = Some(read_le_u32(item, 4)?);
        } else if item_id == VIRTUAL_DISK_SIZE_GUID {
            if item.len() < 8 {
                return None;
            }
            virtual_disk_size = Some(read_le_u64(item, 0)?);
        } else if item_id == LOGICAL_SECTOR_SIZE_GUID {
            if item.len() < 4 {
                return None;
            }
            logical_sector_size = Some(read_le_u32(item, 0)?);
        } else if item_id == PHYSICAL_SECTOR_SIZE_GUID {
            if item.len() < 4 {
                return None;
            }
            physical_sector_size = Some(read_le_u32(item, 0)?);
        }
    }

    let block_size = block_size?;
    let file_parameters = file_parameters?;
    let virtual_disk_size = virtual_disk_size?;
    let logical_sector_size = logical_sector_size?;
    let physical_sector_size = physical_sector_size?;

    if virtual_disk_size == 0
        || !block_size.is_power_of_two()
        || !(VHDX_MIN_PAYLOAD_BLOCK_SIZE..=VHDX_MAX_PAYLOAD_BLOCK_SIZE).contains(&block_size)
        || !matches!(logical_sector_size, 512 | 4096)
        || !matches!(physical_sector_size, 512 | 4096)
    {
        return None;
    }

    Some(VhdxMetadata {
        virtual_disk_size,
        block_size,
        logical_sector_size,
        physical_sector_size,
        has_parent: file_parameters & VHDX_FILE_PARAMETERS_HAS_PARENT != 0,
    })
}

pub fn payload_block_count(virtual_disk_size: u64, block_size: u32) -> Option<u64> {
    div_round_up(virtual_disk_size, u64::from(block_size))
}

pub fn bat_entry_count(payload_blocks: u64, chunk_ratio: u64) -> Option<u64> {
    if payload_blocks == 0 || chunk_ratio == 0 {
        return None;
    }

    payload_blocks.checked_add((payload_blocks - 1) / chunk_ratio)
}

pub fn payload_bat_index(payload_block_index: u64, chunk_ratio: u64) -> Option<u64> {
    if chunk_ratio == 0 {
        return None;
    }

    payload_block_index.checked_add(payload_block_index / chunk_ratio)
}

pub fn parse_bat_entry(raw: u64) -> VhdxBatEntry {
    VhdxBatEntry {
        state: (raw & 0x7) as u8,
        file_offset: (raw >> 20) * VHDX_MIB,
    }
}

pub fn read_le_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub fn read_le_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn parse_region_table_at(header_section: &[u8], offset: usize) -> Option<VhdxRegions> {
    let table = header_section.get(offset..offset.checked_add(VHDX_REGION_TABLE_SIZE)?)?;
    parse_region_table(table)
}

fn parse_region_table(table: &[u8]) -> Option<VhdxRegions> {
    if table.len() < 16 || table.get(0..4)? != VHDX_REGION_SIGNATURE {
        return None;
    }

    let entry_count = read_le_u32(table, 8)?;
    if entry_count > VHDX_MAX_REGION_ENTRIES {
        return None;
    }

    let mut bat = None;
    let mut metadata = None;

    for index in 0..usize::try_from(entry_count).ok()? {
        let entry_offset = 16usize.checked_add(index.checked_mul(32)?)?;
        let entry = table.get(entry_offset..entry_offset.checked_add(32)?)?;
        let guid = entry.get(0..16)?;
        let file_offset = read_le_u64(entry, 16)?;
        let length = u64::from(read_le_u32(entry, 24)?);
        let required = read_le_u32(entry, 28)? & 1 != 0;

        if length == 0 {
            return None;
        }

        if guid == BAT_REGION_GUID {
            bat = Some((file_offset, length));
        } else if guid == METADATA_REGION_GUID {
            metadata = Some((file_offset, length));
        } else if required {
            return None;
        }
    }

    let (bat_offset, bat_length) = bat?;
    let (metadata_offset, metadata_length) = metadata?;
    Some(VhdxRegions {
        bat_offset,
        bat_length,
        metadata_offset,
        metadata_length,
    })
}

fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }

    value.checked_add(divisor - 1).map(|value| value / divisor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn parses_region_table_from_header_section() {
        let header = make_header_section();
        let regions = parse_vhdx_regions(&header).expect("regions");

        assert_eq!(regions.bat_offset, 2 * VHDX_MIB);
        assert_eq!(regions.bat_length, 1024 * 1024);
        assert_eq!(regions.metadata_offset, VHDX_MIB);
        assert_eq!(regions.metadata_length, 1024 * 1024);
    }

    #[test]
    fn parses_required_metadata_items() {
        let metadata = make_metadata_region(false, 4096);
        let parsed = parse_vhdx_metadata(&metadata).expect("metadata");

        assert_eq!(parsed.virtual_disk_size, 64 * VHDX_MIB);
        assert_eq!(parsed.block_size, 2 * 1024 * 1024);
        assert_eq!(parsed.logical_sector_size, 4096);
        assert_eq!(parsed.physical_sector_size, 4096);
        assert!(!parsed.has_parent);
        assert_eq!(parsed.chunk_ratio(), Some(16_384));
    }

    #[test]
    fn detects_parent_vhdx_metadata() {
        let metadata = make_metadata_region(true, 512);
        let parsed = parse_vhdx_metadata(&metadata).expect("metadata");

        assert!(parsed.has_parent);
        assert_eq!(parsed.logical_sector_size, 512);
    }

    #[test]
    fn calculates_interleaved_bat_entry_count() {
        assert_eq!(bat_entry_count(4, 4), Some(4));
        assert_eq!(bat_entry_count(5, 4), Some(6));
        assert_eq!(payload_bat_index(0, 4), Some(0));
        assert_eq!(payload_bat_index(3, 4), Some(3));
        assert_eq!(payload_bat_index(4, 4), Some(5));
    }

    #[test]
    fn parses_bat_entry_state_and_offset() {
        let raw = (3u64 << 20) | u64::from(VHDX_BAT_STATE_FULLY_PRESENT);
        let entry = parse_bat_entry(raw);

        assert_eq!(entry.state, VHDX_BAT_STATE_FULLY_PRESENT);
        assert_eq!(entry.file_offset, 3 * VHDX_MIB);
    }

    fn make_header_section() -> Vec<u8> {
        let mut header = vec![0u8; VHDX_HEADER_SECTION_SIZE];
        header[0..8].copy_from_slice(VHDX_FILE_IDENTIFIER);

        let table = &mut header
            [VHDX_REGION_TABLE_1_OFFSET..VHDX_REGION_TABLE_1_OFFSET + VHDX_REGION_TABLE_SIZE];
        table[0..4].copy_from_slice(VHDX_REGION_SIGNATURE);
        write_le_u32(table, 8, 2);
        write_region_entry(table, 16, BAT_REGION_GUID, 2 * VHDX_MIB, 1024 * 1024, 1);
        write_region_entry(table, 48, METADATA_REGION_GUID, VHDX_MIB, 1024 * 1024, 1);

        header
    }

    fn make_metadata_region(has_parent: bool, logical_sector_size: u32) -> Vec<u8> {
        let mut metadata = vec![0u8; 0x10100];
        metadata[0..8].copy_from_slice(VHDX_METADATA_SIGNATURE);
        write_le_u16(&mut metadata, 10, 4);

        write_metadata_entry(&mut metadata, 32, FILE_PARAMETERS_GUID, 0x10000, 8);
        write_le_u32(&mut metadata, 0x10000, 2 * 1024 * 1024);
        write_le_u32(
            &mut metadata,
            0x10004,
            if has_parent {
                VHDX_FILE_PARAMETERS_HAS_PARENT
            } else {
                0
            },
        );

        write_metadata_entry(&mut metadata, 64, VIRTUAL_DISK_SIZE_GUID, 0x10008, 8);
        write_le_u64(&mut metadata, 0x10008, 64 * VHDX_MIB);

        write_metadata_entry(&mut metadata, 96, LOGICAL_SECTOR_SIZE_GUID, 0x10010, 4);
        write_le_u32(&mut metadata, 0x10010, logical_sector_size);

        write_metadata_entry(&mut metadata, 128, PHYSICAL_SECTOR_SIZE_GUID, 0x10014, 4);
        write_le_u32(&mut metadata, 0x10014, 4096);

        metadata
    }

    fn write_region_entry(
        table: &mut [u8],
        offset: usize,
        guid: [u8; 16],
        file_offset: u64,
        length: u32,
        flags: u32,
    ) {
        table[offset..offset + 16].copy_from_slice(&guid);
        write_le_u64(table, offset + 16, file_offset);
        write_le_u32(table, offset + 24, length);
        write_le_u32(table, offset + 28, flags);
    }

    fn write_metadata_entry(
        metadata: &mut [u8],
        offset: usize,
        guid: [u8; 16],
        item_offset: u32,
        length: u32,
    ) {
        metadata[offset..offset + 16].copy_from_slice(&guid);
        write_le_u32(metadata, offset + 16, item_offset);
        write_le_u32(metadata, offset + 20, length);
        write_le_u32(metadata, offset + 24, 0x6);
    }

    fn write_le_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
