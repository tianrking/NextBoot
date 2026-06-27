use super::super::*;
use alloc::vec::Vec;

pub(super) fn make_wim_header(
    flags: u32,
    image_count: u32,
    boot_index: u32,
) -> [u8; WIM_HEADER_SIZE] {
    let mut header = [0u8; WIM_HEADER_SIZE];
    header[0..8].copy_from_slice(WIM_SIGNATURE);
    write_le_u32(&mut header, HEADER_LEN_OFFSET, WIM_HEADER_SIZE as u32);
    write_le_u32(&mut header, VERSION_OFFSET, 0x0000_0d00);
    write_le_u32(&mut header, FLAGS_OFFSET, flags);
    write_le_u32(&mut header, CHUNK_LEN_OFFSET, 32 * 1024);
    header[24..40].copy_from_slice(&[0x42; 16]);
    write_le_u16(&mut header, PART_OFFSET, 1);
    write_le_u16(&mut header, PARTS_OFFSET, 1);
    write_le_u32(&mut header, IMAGE_COUNT_OFFSET, image_count);
    write_resource_header(
        &mut header,
        LOOKUP_RESOURCE_OFFSET,
        0x1000,
        WIM_RESHDR_METADATA | WIM_RESHDR_COMPRESSED,
        0x4000,
        0x2000,
    );
    write_resource_header(&mut header, XML_RESOURCE_OFFSET, 0x800, 0, 0x6000, 0x1200);
    write_resource_header(
        &mut header,
        BOOT_RESOURCE_OFFSET,
        0x200,
        WIM_RESHDR_METADATA,
        0x8000,
        0x400,
    );
    write_le_u32(&mut header, BOOT_INDEX_OFFSET, boot_index);
    write_resource_header(&mut header, INTEGRITY_RESOURCE_OFFSET, 0, 0, 0, 0);
    header
}

pub(super) fn make_wim_metadata_with_chunk_len(chunk_len: u32) -> WimMetadata {
    make_wim_metadata_with_flags(WIM_HDR_XPRESS, chunk_len)
}

pub(super) fn make_wim_metadata_with_flags(flags: u32, chunk_len: u32) -> WimMetadata {
    let mut header = make_wim_header(flags, 1, 1);
    write_le_u32(&mut header, CHUNK_LEN_OFFSET, chunk_len);
    parse_wim_metadata(&header).expect("metadata")
}

pub(super) fn make_xpress_literal_stream(bytes: &[u8]) -> Vec<u8> {
    let mut out = alloc::vec![0x99u8; XPRESS_LENGTH_TABLE_SIZE];
    let mut bits = Vec::new();
    for byte in bytes {
        push_bits(&mut bits, u16::from(*byte), 9);
    }
    push_bits(&mut bits, XPRESS_END_MARKER, 9);
    while bits.len() < 32 || bits.len() % 16 != 0 {
        bits.push(0);
    }
    for _ in 0..16 {
        bits.push(0);
    }

    for chunk in bits.chunks_exact(16) {
        let mut word = 0u16;
        for bit in chunk {
            word = (word << 1) | u16::from(*bit);
        }
        out.extend_from_slice(&word.to_le_bytes());
    }

    out
}

pub(super) fn make_sparse_xpress_literal_stream(byte: u8) -> Vec<u8> {
    let mut out = alloc::vec![0u8; XPRESS_LENGTH_TABLE_SIZE];
    set_xpress_code_len(&mut out, usize::from(byte), 1);
    set_xpress_code_len(&mut out, usize::from(XPRESS_END_MARKER), 1);
    out.extend_from_slice(&0x4000u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

pub(super) fn set_xpress_code_len(lengths: &mut [u8], symbol: usize, len: u8) {
    let slot = &mut lengths[symbol / 2];
    if symbol % 2 == 0 {
        *slot = (*slot & 0xf0) | (len & 0x0f);
    } else {
        *slot = (*slot & 0x0f) | ((len & 0x0f) << 4);
    }
}

pub(super) fn make_lzx_uncompressed_block(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::new();
    push_bits(&mut bits, 3, 3);
    push_bits(&mut bits, 0, 1);
    push_bits(&mut bits, ((bytes.len() >> 8) & 0xff) as u16, 8);
    push_bits(&mut bits, (bytes.len() & 0xff) as u16, 8);
    push_bits(&mut bits, 0, 1);
    while bits.len() % 16 != 0 {
        bits.push(0);
    }

    let mut out = bits_to_le_words(&bits);
    for _ in 0..3 {
        out.extend_from_slice(&1u32.to_le_bytes());
    }
    out.extend_from_slice(bytes);
    if bytes.len() % 2 != 0 {
        out.push(0);
    }
    out
}

pub(super) fn push_bits(bits: &mut Vec<u8>, value: u16, count: usize) {
    for index in (0..count).rev() {
        bits.push(((value >> index) & 1) as u8);
    }
}

pub(super) fn bits_to_le_words(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in bits.chunks_exact(16) {
        let mut word = 0u16;
        for bit in chunk {
            word = (word << 1) | u16::from(*bit);
        }
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_lookup_entry(
    data: &mut [u8],
    offset: usize,
    compressed_size: u64,
    flags: u8,
    resource_offset: u64,
    uncompressed_size: u64,
    part: u16,
    ref_count: u32,
    hash: &[u8; WIM_HASH_SIZE],
) {
    write_resource_header(
        data,
        offset,
        compressed_size,
        flags,
        resource_offset,
        uncompressed_size,
    );
    write_le_u16(data, offset + LOOKUP_ENTRY_PART_OFFSET, part);
    write_le_u32(data, offset + LOOKUP_ENTRY_REFCNT_OFFSET, ref_count);
    data[offset + LOOKUP_ENTRY_HASH_OFFSET..offset + WIM_LOOKUP_ENTRY_SIZE].copy_from_slice(hash);
}

pub(super) fn write_directory_entry(
    data: &mut [u8],
    offset: usize,
    name: &str,
    subdir: usize,
    hash: &[u8; WIM_HASH_SIZE],
) -> usize {
    let name_len = name.len() * 2;
    let len = align_up_8(WIM_DIRECTORY_ENTRY_FIXED_SIZE + name_len).expect("entry length");
    write_le_u64(data, offset, len as u64);
    write_le_u32(
        data,
        offset + DIRECTORY_ENTRY_ATTRIBUTES_OFFSET,
        WIM_ATTR_NORMAL,
    );
    write_le_u32(
        data,
        offset + DIRECTORY_ENTRY_SECURITY_OFFSET,
        WIM_NO_SECURITY,
    );
    write_le_u64(data, offset + DIRECTORY_ENTRY_SUBDIR_OFFSET, subdir as u64);
    data[offset + DIRECTORY_ENTRY_HASH_OFFSET
        ..offset + DIRECTORY_ENTRY_HASH_OFFSET + WIM_HASH_SIZE]
        .copy_from_slice(hash);
    write_le_u16(
        data,
        offset + DIRECTORY_ENTRY_NAME_LEN_OFFSET,
        name_len as u16,
    );

    let mut name_offset = offset + WIM_DIRECTORY_ENTRY_FIXED_SIZE;
    for byte in name.bytes() {
        write_le_u16(data, name_offset, u16::from(byte));
        name_offset += 2;
    }

    len
}

pub(super) fn write_resource_header(
    data: &mut [u8],
    offset: usize,
    compressed_size: u64,
    flags: u8,
    resource_offset: u64,
    uncompressed_size: u64,
) {
    let zlen_flags = compressed_size | (u64::from(flags) << 56);
    write_le_u64(data, offset, zlen_flags);
    write_le_u64(data, offset + 8, resource_offset);
    write_le_u64(data, offset + 16, uncompressed_size);
}

pub(super) fn write_le_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn align_up_8(value: usize) -> Option<usize> {
    value.checked_add(7).map(|value| value & !7)
}
