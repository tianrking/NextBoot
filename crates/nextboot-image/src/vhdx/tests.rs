use super::*;
use crate::{ImagePlanError, ImageSpan, ImageSpanSource};
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
    let mut metadata = make_metadata_region(true, 512);
    append_parent_locator(
        &mut metadata,
        &[
            (
                VHDX_PARENT_KEY_PARENT_LINKAGE,
                "{83ed0ec3-24c8-49a6-a959-5e4bf1288bfb}",
            ),
            (VHDX_PARENT_KEY_RELATIVE_PATH, "..\\base.vhdx"),
            (
                VHDX_PARENT_KEY_VOLUME_PATH,
                "\\??\\Volume{nextboot}\\base.vhdx",
            ),
            (VHDX_PARENT_KEY_ABSOLUTE_WIN32_PATH, "C:\\images\\base.vhdx"),
        ],
    );
    let parsed = parse_vhdx_metadata(&metadata).expect("metadata");

    assert!(parsed.has_parent);
    assert_eq!(parsed.logical_sector_size, 512);

    let locator = parsed.parent_locator.as_ref().expect("parent locator");
    assert_eq!(
        locator.get(VHDX_PARENT_KEY_RELATIVE_PATH),
        Some("..\\base.vhdx")
    );
    assert_eq!(
        parsed.parent_paths(),
        vec![
            "..\\base.vhdx",
            "\\??\\Volume{nextboot}\\base.vhdx",
            "C:\\images\\base.vhdx"
        ]
    );
    assert_eq!(
        parsed.parent_linkages(),
        vec!["{83ed0ec3-24c8-49a6-a959-5e4bf1288bfb}"]
    );
}

#[test]
fn rejects_malformed_parent_locator() {
    let mut locator = parent_locator_region(&[(VHDX_PARENT_KEY_RELATIVE_PATH, "base.vhdx")]);
    locator[18..20].copy_from_slice(&2u16.to_le_bytes());

    assert!(parse_vhdx_parent_locator(&locator).is_none());
}

#[test]
fn calculates_interleaved_bat_entry_count() {
    assert_eq!(bat_entry_count(4, 4), Some(5));
    assert_eq!(bat_entry_count(5, 4), Some(10));
    assert_eq!(payload_bat_index(0, 4), Some(0));
    assert_eq!(payload_bat_index(3, 4), Some(3));
    assert_eq!(payload_bat_index(4, 4), Some(5));
    assert_eq!(sector_bitmap_bat_index(0, 4), Some(4));
    assert_eq!(sector_bitmap_bat_index(3, 4), Some(4));
    assert_eq!(sector_bitmap_bat_index(4, 4), Some(9));
}

#[test]
fn parses_bat_entry_state_and_offset() {
    let raw = (3u64 << 20) | u64::from(VHDX_BAT_STATE_FULLY_PRESENT);
    let entry = parse_bat_entry(raw);

    assert_eq!(entry.state, VHDX_BAT_STATE_FULLY_PRESENT);
    assert_eq!(entry.file_offset, 3 * VHDX_MIB);
}

#[test]
fn plans_parent_required_vhdx_spans() {
    let mut metadata = parse_vhdx_metadata(&make_metadata_region(true, 512)).expect("metadata");
    metadata.virtual_disk_size = 4 * metadata.block_size as u64;
    let chunk_ratio = metadata.chunk_ratio().expect("chunk ratio");
    let mut bat = vec![0u8; ((chunk_ratio + 1) * 8) as usize];
    write_payload_bat(
        &mut bat,
        0,
        chunk_ratio,
        VHDX_BAT_STATE_FULLY_PRESENT,
        3 * VHDX_MIB,
    );
    write_payload_bat(&mut bat, 1, chunk_ratio, VHDX_BAT_STATE_NOT_PRESENT, 0);
    write_payload_bat(&mut bat, 2, chunk_ratio, VHDX_BAT_STATE_ZERO, 0);
    write_payload_bat(
        &mut bat,
        3,
        chunk_ratio,
        VHDX_BAT_STATE_PARTIALLY_PRESENT,
        5 * VHDX_MIB,
    );

    let spans = plan_vhdx_spans(&metadata, &bat, |_| Ok(false)).expect("plan");

    assert_eq!(
        spans,
        vec![
            ImageSpan {
                virtual_offset: 0,
                byte_count: u64::from(metadata.block_size),
                source: ImageSpanSource::Image {
                    file_offset: 3 * VHDX_MIB
                }
            },
            ImageSpan {
                virtual_offset: u64::from(metadata.block_size),
                byte_count: u64::from(metadata.block_size),
                source: ImageSpanSource::Parent
            },
            ImageSpan {
                virtual_offset: 2 * u64::from(metadata.block_size),
                byte_count: u64::from(metadata.block_size),
                source: ImageSpanSource::Zero
            },
            ImageSpan {
                virtual_offset: 3 * u64::from(metadata.block_size),
                byte_count: u64::from(metadata.block_size),
                source: ImageSpanSource::Parent
            }
        ]
    );
}

#[test]
fn plans_self_contained_partial_vhdx_block_as_child_image() {
    let mut metadata = parse_vhdx_metadata(&make_metadata_region(true, 512)).expect("metadata");
    metadata.virtual_disk_size = u64::from(metadata.block_size);
    let chunk_ratio = metadata.chunk_ratio().expect("chunk ratio");
    let mut bat = vec![0u8; ((chunk_ratio + 1) * 8) as usize];
    write_payload_bat(
        &mut bat,
        0,
        chunk_ratio,
        VHDX_BAT_STATE_PARTIALLY_PRESENT,
        6 * VHDX_MIB,
    );

    let spans = plan_vhdx_spans(&metadata, &bat, |_| Ok(true)).expect("plan");

    assert_eq!(
        spans,
        vec![ImageSpan {
            virtual_offset: 0,
            byte_count: u64::from(metadata.block_size),
            source: ImageSpanSource::Image {
                file_offset: 6 * VHDX_MIB
            }
        }]
    );
}

#[test]
fn rejects_partial_vhdx_parent_reference_without_parent_metadata() {
    let mut metadata = parse_vhdx_metadata(&make_metadata_region(false, 512)).expect("metadata");
    metadata.virtual_disk_size = u64::from(metadata.block_size);
    let chunk_ratio = metadata.chunk_ratio().expect("chunk ratio");
    let mut bat = vec![0u8; ((chunk_ratio + 1) * 8) as usize];
    write_payload_bat(
        &mut bat,
        0,
        chunk_ratio,
        VHDX_BAT_STATE_PARTIALLY_PRESENT,
        6 * VHDX_MIB,
    );

    assert_eq!(
        plan_vhdx_spans(&metadata, &bat, |_| Ok(false)),
        Err(ImagePlanError::Unsupported)
    );
}

#[test]
fn resolves_same_volume_parent_paths() {
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/children/diff.vhdx", "..\\base.vhdx"),
        Some("/ISO/base.vhdx".into())
    );
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/children/diff.vhdx", "\\parents\\base.vhdx"),
        Some("/parents/base.vhdx".into())
    );
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/diff.vhdx", "..\\base.vhdx"),
        Some("/base.vhdx".into())
    );
    assert_eq!(
        resolve_same_volume_parent_path("/diff.vhdx", "..\\base.vhdx"),
        None
    );
    assert_eq!(
        resolve_same_volume_parent_path("ISO/diff.vhdx", "base.vhdx"),
        None
    );
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/diff.vhdx", "C:\\base.vhdx"),
        None
    );
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/diff.vhdx", "\\\\??\\Volume{abc}\\base.vhdx"),
        None
    );
    assert_eq!(
        resolve_same_volume_parent_path("/ISO/diff.vhdx", "\\\\server\\share\\base.vhdx"),
        None
    );
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
    let mut metadata = vec![0u8; 0x20000];
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

fn append_parent_locator(metadata: &mut [u8], entries: &[(&str, &str)]) {
    let locator = parent_locator_region(entries);
    write_le_u16(metadata, 10, 5);
    write_metadata_entry(
        metadata,
        160,
        PARENT_LOCATOR_METADATA_GUID,
        0x10018,
        locator.len() as u32,
    );
    metadata[0x10018..0x10018 + locator.len()].copy_from_slice(&locator);
}

fn parent_locator_region(entries: &[(&str, &str)]) -> Vec<u8> {
    let table_bytes = entries.len() * 12;
    let mut locator = vec![0u8; 20 + table_bytes];
    locator[0..16].copy_from_slice(&VHDX_PARENT_LOCATOR_TYPE_GUID);
    write_le_u16(&mut locator, 18, entries.len() as u16);

    for (index, (key, value)) in entries.iter().enumerate() {
        let table_offset = 20 + index * 12;
        let key_offset = locator.len();
        append_utf16le(&mut locator, key);
        let value_offset = locator.len();
        append_utf16le(&mut locator, value);

        write_le_u32(&mut locator, table_offset, key_offset as u32);
        write_le_u32(&mut locator, table_offset + 4, value_offset as u32);
        write_le_u16(&mut locator, table_offset + 8, (key.len() * 2) as u16);
        write_le_u16(&mut locator, table_offset + 10, (value.len() * 2) as u16);
    }

    locator
}

fn append_utf16le(out: &mut Vec<u8>, text: &str) {
    for code_unit in text.encode_utf16() {
        out.extend_from_slice(&code_unit.to_le_bytes());
    }
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

fn write_payload_bat(
    bat: &mut [u8],
    payload_index: u64,
    chunk_ratio: u64,
    state: u8,
    file_offset: u64,
) {
    let bat_index = payload_bat_index(payload_index, chunk_ratio).expect("bat index");
    let raw = ((file_offset / VHDX_MIB) << 20) | u64::from(state);
    write_le_u64(bat, (bat_index * 8) as usize, raw);
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
