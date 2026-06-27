use super::*;

mod support;
use support::*;

use super::*;

#[test]
fn parses_bootable_lzx_wim_header() {
    let header = make_wim_header(WIM_HDR_LZX, 2, 1);

    let metadata = parse_wim_metadata(&header).unwrap();
    assert_eq!(metadata.header_len, WIM_HEADER_SIZE as u32);
    assert_eq!(metadata.compression, WimCompression::Lzx);
    assert_eq!(metadata.image_count, 2);
    assert_eq!(metadata.boot_index, 1);
    assert!(metadata.is_bootable());
    assert!(metadata.boot_index_in_range());
    assert!(metadata.is_wimboot_supported());
    assert!(metadata.lookup.is_metadata());
    assert!(metadata.lookup.is_compressed());
    assert_eq!(metadata.lookup.file_end(), Some(0x5000));
}

#[test]
fn marks_lzms_esd_as_not_supported_for_wimboot() {
    let header = make_wim_header(WIM_HDR_LZMS, 1, 1);
    let metadata = parse_wim_metadata(&header).unwrap();

    assert_eq!(metadata.compression, WimCompression::Lzms);
    assert!(metadata.is_bootable());
    assert!(!metadata.is_wimboot_supported());
}

#[test]
fn treats_zero_boot_index_as_non_bootable() {
    let header = make_wim_header(WIM_HDR_XPRESS, 3, 0);
    let metadata = parse_wim_metadata(&header).unwrap();

    assert_eq!(metadata.compression, WimCompression::Xpress);
    assert!(!metadata.is_bootable());
    assert!(!metadata.boot_index_in_range());
    assert!(!metadata.is_wimboot_supported());
}

#[test]
fn does_not_support_out_of_range_boot_index() {
    let header = make_wim_header(WIM_HDR_XPRESS, 1, 2);
    let metadata = parse_wim_metadata(&header).unwrap();

    assert!(metadata.is_bootable());
    assert!(!metadata.boot_index_in_range());
    assert!(!metadata.is_wimboot_supported());
}

#[test]
fn rejects_bad_signature_and_invalid_header_values() {
    let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
    header[0] = b'X';
    assert!(parse_wim_metadata(&header).is_none());

    let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
    write_le_u32(&mut header, CHUNK_LEN_OFFSET, 0);
    assert!(parse_wim_metadata(&header).is_none());

    let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
    write_le_u16(&mut header, PART_OFFSET, 2);
    write_le_u16(&mut header, PARTS_OFFSET, 1);
    assert!(parse_wim_metadata(&header).is_none());
}

#[test]
fn selects_metadata_resources_from_lookup_table() {
    let header = make_wim_header(WIM_HDR_XPRESS, 2, 1);
    let metadata = parse_wim_metadata(&header).unwrap();
    let mut lookup = [0u8; WIM_LOOKUP_ENTRY_SIZE * 3];
    let first_hash = [0x11; WIM_HASH_SIZE];
    let file_hash = [0x22; WIM_HASH_SIZE];
    let second_hash = [0x33; WIM_HASH_SIZE];

    write_lookup_entry(
        &mut lookup,
        0,
        0x120,
        WIM_RESHDR_METADATA,
        0x10_000,
        0x400,
        1,
        1,
        &first_hash,
    );
    write_lookup_entry(
        &mut lookup,
        WIM_LOOKUP_ENTRY_SIZE,
        0x80,
        0,
        0x20_000,
        0x80,
        1,
        1,
        &file_hash,
    );
    write_lookup_entry(
        &mut lookup,
        WIM_LOOKUP_ENTRY_SIZE * 2,
        0x130,
        WIM_RESHDR_METADATA,
        0x30_000,
        0x500,
        1,
        1,
        &second_hash,
    );

    assert_eq!(metadata_resource_count(&lookup), 2);
    assert_eq!(
        metadata_resource_for_image(&metadata, &lookup, 0),
        Some(metadata.boot)
    );
    assert_eq!(
        metadata_resource_for_image(&metadata, &lookup, 1)
            .expect("first image metadata")
            .offset,
        0x10_000
    );
    assert_eq!(
        metadata_resource_for_image(&metadata, &lookup, 2)
            .expect("second image metadata")
            .offset,
        0x30_000
    );
    assert_eq!(metadata_resource_for_image(&metadata, &lookup, 3), None);
    assert_eq!(
        lookup_resource_for_hash(&lookup, &file_hash)
            .expect("file resource")
            .offset,
        0x20_000
    );
}

#[test]
fn finds_file_resource_by_ascii_path() {
    let mut metadata = [0u8; 512];
    let root_offset = WIM_SECURITY_HEADER_SIZE;
    let windows_dir_offset = 160usize;
    let system32_dir_offset = 320usize;
    let file_hash = [0x7b; WIM_HASH_SIZE];
    let empty_hash = [0u8; WIM_HASH_SIZE];

    write_le_u32(&mut metadata, 0, 0);
    write_le_u32(&mut metadata, 4, 0);
    let windows_len = write_directory_entry(
        &mut metadata,
        root_offset,
        "Windows",
        windows_dir_offset,
        &empty_hash,
    );
    write_le_u64(&mut metadata, root_offset + windows_len, 0);

    let system32_len = write_directory_entry(
        &mut metadata,
        windows_dir_offset,
        "System32",
        system32_dir_offset,
        &empty_hash,
    );
    write_le_u64(&mut metadata, windows_dir_offset + system32_len, 0);

    let winpeshl_len = write_directory_entry(
        &mut metadata,
        system32_dir_offset,
        "winpeshl.exe",
        0,
        &file_hash,
    );
    write_le_u64(&mut metadata, system32_dir_offset + winpeshl_len, 0);

    let mut lookup = [0u8; WIM_LOOKUP_ENTRY_SIZE];
    write_lookup_entry(&mut lookup, 0, 0x200, 0, 0x44_000, 0x200, 1, 1, &file_hash);

    let entry =
        find_path_entry(&metadata, "/windows/system32/WINPESHL.EXE").expect("winpeshl entry");
    assert_eq!(entry.offset, system32_dir_offset);
    assert_eq!(entry.entry.hash, file_hash);
    assert_eq!(
        file_resource_for_path(&metadata, &lookup, "\\Windows\\System32\\winpeshl.exe")
            .expect("winpeshl resource")
            .offset,
        0x44_000
    );
    assert_eq!(
        find_path_entry(&metadata, "\\Windows\\System32\\missing.exe"),
        Err(WimPathError::NotFound)
    );
}

#[test]
fn reads_uncompressed_resource_ranges() {
    let metadata = make_wim_metadata_with_chunk_len(4);
    let mut wim_file = [0u8; 16];
    wim_file[4..12].copy_from_slice(b"abcdefgh");
    let resource = WimResourceHeader {
        compressed_size: 8,
        flags: 0,
        offset: 4,
        uncompressed_size: 8,
    };
    let mut out = [0u8; 4];

    read_resource_range(&metadata, &wim_file, &resource, 2, &mut out).expect("resource range");

    assert_eq!(&out, b"cdef");
}

#[test]
fn reads_stored_chunks_from_compressed_resource() {
    let metadata = make_wim_metadata_with_chunk_len(4);
    let mut wim_file = [0u8; 64];
    let resource = WimResourceHeader {
        compressed_size: 18,
        flags: WIM_RESHDR_COMPRESSED,
        offset: 16,
        uncompressed_size: 10,
    };

    write_le_u32(&mut wim_file, 16, 4);
    write_le_u32(&mut wim_file, 20, 8);
    wim_file[24..28].copy_from_slice(b"abcd");
    wim_file[28..32].copy_from_slice(b"efgh");
    wim_file[32..34].copy_from_slice(b"ij");
    let mut out = [0u8; 6];

    read_resource_range(&metadata, &wim_file, &resource, 2, &mut out).expect("stored chunks");

    assert_eq!(&out, b"cdefgh");
}

#[test]
fn decompresses_xpress_literal_stream() {
    let compressed = make_xpress_literal_stream(b"next");
    let mut out = [0u8; 4];

    assert_eq!(decompress_xpress(&compressed, &mut out), Ok(4));
    assert_eq!(&out, b"next");
}

#[test]
fn decompresses_sparse_xpress_literal_stream() {
    let compressed = make_sparse_xpress_literal_stream(b'A');
    let mut out = [0u8; 1];

    assert_eq!(decompress_xpress(&compressed, &mut out), Ok(1));
    assert_eq!(&out, b"A");
}

#[test]
fn reads_xpress_chunks_from_compressed_resource() {
    let metadata = make_wim_metadata_with_chunk_len(4);
    let compressed = make_xpress_literal_stream(b"wxyz");
    let mut wim_file = alloc::vec![0u8; 16 + compressed.len()];
    wim_file[16..].copy_from_slice(&compressed);
    let resource = WimResourceHeader {
        compressed_size: compressed.len() as u64,
        flags: WIM_RESHDR_COMPRESSED,
        offset: 16,
        uncompressed_size: 4,
    };
    let mut out = [0u8; 2];

    read_resource_range(&metadata, &wim_file, &resource, 1, &mut out).expect("xpress chunk");

    assert_eq!(&out, b"xy");
}

#[test]
fn reads_compressed_resource_with_random_access_reader() {
    let metadata = make_wim_metadata_with_chunk_len(4);
    let compressed = make_xpress_literal_stream(b"wxyz");
    let mut wim_file = alloc::vec![0u8; 16 + compressed.len()];
    wim_file[16..].copy_from_slice(&compressed);
    let resource = WimResourceHeader {
        compressed_size: compressed.len() as u64,
        flags: WIM_RESHDR_COMPRESSED,
        offset: 16,
        uncompressed_size: 4,
    };
    let mut out = [0u8; 3];
    let mut calls = 0usize;

    read_resource_range_with(
        &metadata,
        wim_file.len() as u64,
        &resource,
        1,
        &mut out,
        |offset, buf| {
            calls += 1;
            let start = usize::try_from(offset).map_err(|_| WimReadError::ResourceOutOfBounds)?;
            let end = start
                .checked_add(buf.len())
                .ok_or(WimReadError::ResourceOutOfBounds)?;
            buf.copy_from_slice(
                wim_file
                    .get(start..end)
                    .ok_or(WimReadError::ResourceOutOfBounds)?,
            );
            Ok(())
        },
    )
    .expect("reader-backed xpress resource");

    assert_eq!(&out, b"xyz");
    assert!(calls > 0);
}

#[test]
fn reads_lzx_chunks_from_compressed_resource() {
    let metadata = make_wim_metadata_with_flags(WIM_HDR_LZX, 32);
    let compressed = make_lzx_uncompressed_block(b"lzx-data");
    let mut wim_file = alloc::vec![0u8; 16 + compressed.len()];
    wim_file[16..].copy_from_slice(&compressed);
    let resource = WimResourceHeader {
        compressed_size: compressed.len() as u64,
        flags: WIM_RESHDR_COMPRESSED,
        offset: 16,
        uncompressed_size: 8,
    };
    let mut out = [0u8; 4];

    read_resource_range(&metadata, &wim_file, &resource, 4, &mut out).expect("lzx chunk");

    assert_eq!(&out, b"data");
}

#[test]
fn rejects_invalid_compressed_resource_chunk_tables() {
    let metadata = make_wim_metadata_with_chunk_len(4);
    let wim_file = [0u8; 64];
    let resource = WimResourceHeader {
        compressed_size: 4,
        flags: WIM_RESHDR_COMPRESSED,
        offset: 16,
        uncompressed_size: 10,
    };
    let mut out = [0u8; 1];

    assert_eq!(
        read_resource_range(&metadata, &wim_file, &resource, 0, &mut out),
        Err(WimReadError::InvalidChunkTable)
    );
}
