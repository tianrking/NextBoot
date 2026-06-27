use super::{
    read_le_u16, read_le_u32, read_le_u64, WimCompression, WimLookupEntry, WimMetadata,
    WimResourceHeader, BOOT_INDEX_OFFSET, BOOT_RESOURCE_OFFSET, CHUNK_LEN_OFFSET, FLAGS_OFFSET,
    HEADER_LEN_OFFSET, IMAGE_COUNT_OFFSET, INTEGRITY_RESOURCE_OFFSET, LOOKUP_ENTRY_HASH_OFFSET,
    LOOKUP_ENTRY_PART_OFFSET, LOOKUP_ENTRY_REFCNT_OFFSET, LOOKUP_RESOURCE_OFFSET, PARTS_OFFSET,
    PART_OFFSET, RESOURCE_HEADER_SIZE, VERSION_OFFSET, WIM_HASH_SIZE, WIM_HDR_LZMS, WIM_HDR_LZX,
    WIM_HDR_XPRESS, WIM_HEADER_SIZE, WIM_LOOKUP_ENTRY_SIZE, WIM_RESHDR_ZLEN_MASK, WIM_SIGNATURE,
    XML_RESOURCE_OFFSET,
};

pub fn parse_lookup_entry(data: &[u8]) -> Option<WimLookupEntry> {
    if data.len() < WIM_LOOKUP_ENTRY_SIZE {
        return None;
    }

    let mut hash = [0u8; WIM_HASH_SIZE];
    hash.copy_from_slice(data.get(LOOKUP_ENTRY_HASH_OFFSET..WIM_LOOKUP_ENTRY_SIZE)?);

    Some(WimLookupEntry {
        resource: parse_resource_header(data, 0)?,
        part: read_le_u16(data, LOOKUP_ENTRY_PART_OFFSET)?,
        ref_count: read_le_u32(data, LOOKUP_ENTRY_REFCNT_OFFSET)?,
        hash,
    })
}

pub fn metadata_resource_count(lookup_table: &[u8]) -> usize {
    lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
        .filter(|entry| entry.resource.is_metadata())
        .count()
}

pub fn metadata_resource_for_image(
    metadata: &WimMetadata,
    lookup_table: &[u8],
    image_index: u32,
) -> Option<WimResourceHeader> {
    if image_index == 0 {
        return Some(metadata.boot);
    }

    let mut found = 0u32;
    for entry in lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
    {
        if entry.resource.is_metadata() {
            found = found.checked_add(1)?;
            if found == image_index {
                return Some(entry.resource);
            }
        }
    }

    None
}

pub fn lookup_resource_for_hash(
    lookup_table: &[u8],
    hash: &[u8; WIM_HASH_SIZE],
) -> Option<WimResourceHeader> {
    lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
        .find(|entry| &entry.hash == hash)
        .map(|entry| entry.resource)
}

pub fn parse_wim_metadata(header: &[u8]) -> Option<WimMetadata> {
    if header.len() < WIM_HEADER_SIZE || header.get(0..8)? != WIM_SIGNATURE {
        return None;
    }

    let header_len = read_le_u32(header, HEADER_LEN_OFFSET)?;
    let version = read_le_u32(header, VERSION_OFFSET)?;
    let flags = read_le_u32(header, FLAGS_OFFSET)?;
    let chunk_len = read_le_u32(header, CHUNK_LEN_OFFSET)?;
    let part = read_le_u16(header, PART_OFFSET)?;
    let parts = read_le_u16(header, PARTS_OFFSET)?;
    let image_count = read_le_u32(header, IMAGE_COUNT_OFFSET)?;
    let boot_index = read_le_u32(header, BOOT_INDEX_OFFSET)?;

    if header_len < WIM_HEADER_SIZE as u32
        || version == 0
        || chunk_len == 0
        || part == 0
        || parts == 0
        || part > parts
        || image_count == 0
    {
        return None;
    }

    Some(WimMetadata {
        header_len,
        version,
        flags,
        compression: compression_from_flags(flags),
        chunk_len,
        part,
        parts,
        image_count,
        boot_index,
        lookup: parse_resource_header(header, LOOKUP_RESOURCE_OFFSET)?,
        xml: parse_resource_header(header, XML_RESOURCE_OFFSET)?,
        boot: parse_resource_header(header, BOOT_RESOURCE_OFFSET)?,
        integrity: parse_resource_header(header, INTEGRITY_RESOURCE_OFFSET)?,
    })
}

pub fn compression_from_flags(flags: u32) -> WimCompression {
    if flags & WIM_HDR_LZMS != 0 {
        WimCompression::Lzms
    } else if flags & WIM_HDR_LZX != 0 {
        WimCompression::Lzx
    } else if flags & WIM_HDR_XPRESS != 0 {
        WimCompression::Xpress
    } else {
        WimCompression::None
    }
}

fn parse_resource_header(data: &[u8], offset: usize) -> Option<WimResourceHeader> {
    let resource = data.get(offset..offset.checked_add(RESOURCE_HEADER_SIZE)?)?;
    let zlen_flags = read_le_u64(resource, 0)?;
    Some(WimResourceHeader {
        compressed_size: zlen_flags & WIM_RESHDR_ZLEN_MASK,
        flags: (zlen_flags >> 56) as u8,
        offset: read_le_u64(resource, 8)?,
        uncompressed_size: read_le_u64(resource, 16)?,
    })
}
