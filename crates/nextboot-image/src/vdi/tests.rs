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
    let mut header = make_vdi_header(VDI_TYPE_DIFFERENCING, 4 * 1024 * 1024, 1024 * 1024, 4, 2);
    header[VDI_LINKAGE_UUID_OFFSET..VDI_LINKAGE_UUID_OFFSET + 16].copy_from_slice(&[0x42; 16]);
    header[VDI_PARENT_MODIFY_UUID_OFFSET..VDI_PARENT_MODIFY_UUID_OFFSET + 16]
        .copy_from_slice(&[0x24; 16]);

    let metadata = parse_vdi_metadata(&header).unwrap();
    assert!(metadata.is_differencing());
    assert_eq!(metadata.virtual_disk_size, 4 * 1024 * 1024);
    assert_eq!(metadata.blocks_allocated, 2);
    assert_eq!(metadata.linkage_uuid, [0x42; 16]);
    assert_eq!(metadata.parent_modify_uuid, [0x24; 16]);
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
