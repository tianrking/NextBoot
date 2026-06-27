use super::{
    lookup_resource_for_hash, read_le_u16, read_le_u32, read_le_u64, WimDirectoryEntry,
    WimDirectoryEntryLocation, WimPathError, WimResourceHeader, DIRECTORY_ENTRY_ATTRIBUTES_OFFSET,
    DIRECTORY_ENTRY_HASH_OFFSET, DIRECTORY_ENTRY_NAME_LEN_OFFSET, DIRECTORY_ENTRY_SECURITY_OFFSET,
    DIRECTORY_ENTRY_SHORT_NAME_LEN_OFFSET, DIRECTORY_ENTRY_STREAMS_OFFSET,
    DIRECTORY_ENTRY_SUBDIR_OFFSET, WIM_DIRECTORY_ENTRY_FIXED_SIZE, WIM_HASH_SIZE,
    WIM_SECURITY_HEADER_SIZE,
};

pub fn find_path_entry(
    metadata: &[u8],
    path: &str,
) -> Result<WimDirectoryEntryLocation, WimPathError> {
    if path.bytes().any(|byte| byte >= 0x80) {
        return Err(WimPathError::NonAsciiPath);
    }

    let mut components = path
        .split(|ch| ch == '\\' || ch == '/')
        .filter(|component| !component.is_empty());
    let mut component = components.next().ok_or(WimPathError::EmptyPath)?;
    let mut dir_offset = root_directory_offset(metadata)?;

    loop {
        let location = find_child_entry(metadata, dir_offset, component)?;
        if let Some(next) = components.next() {
            if location.entry.subdir == 0 {
                return Err(WimPathError::NotFound);
            }
            dir_offset = usize::try_from(location.entry.subdir)
                .map_err(|_| WimPathError::MalformedMetadata)?;
            component = next;
        } else {
            return Ok(location);
        }
    }
}

pub fn file_resource_for_path(
    metadata: &[u8],
    lookup_table: &[u8],
    path: &str,
) -> Result<WimResourceHeader, WimPathError> {
    let location = find_path_entry(metadata, path)?;
    lookup_resource_for_hash(lookup_table, &location.entry.hash)
        .ok_or(WimPathError::ResourceNotFound)
}

fn root_directory_offset(metadata: &[u8]) -> Result<usize, WimPathError> {
    if metadata.len() < WIM_SECURITY_HEADER_SIZE {
        return Err(WimPathError::MalformedMetadata);
    }

    let security_len = read_le_u32(metadata, 0).ok_or(WimPathError::MalformedMetadata)? as usize;
    let offset = if security_len > 0 {
        align_up_8(security_len).ok_or(WimPathError::MalformedMetadata)?
    } else {
        WIM_SECURITY_HEADER_SIZE
    };

    if offset > metadata.len() {
        return Err(WimPathError::MalformedMetadata);
    }

    Ok(offset)
}

fn find_child_entry(
    metadata: &[u8],
    dir_offset: usize,
    name: &str,
) -> Result<WimDirectoryEntryLocation, WimPathError> {
    let mut offset = dir_offset;

    loop {
        let entry_len = read_le_u64(metadata, offset).ok_or(WimPathError::MalformedMetadata)?;
        if entry_len == 0 {
            return Err(WimPathError::NotFound);
        }

        let entry_len = usize::try_from(entry_len).map_err(|_| WimPathError::MalformedMetadata)?;
        let entry_end = offset
            .checked_add(entry_len)
            .ok_or(WimPathError::MalformedMetadata)?;
        if entry_len < WIM_DIRECTORY_ENTRY_FIXED_SIZE || entry_end > metadata.len() {
            return Err(WimPathError::MalformedMetadata);
        }

        let entry = parse_directory_entry(metadata, offset)?;
        let name_start = offset
            .checked_add(WIM_DIRECTORY_ENTRY_FIXED_SIZE)
            .ok_or(WimPathError::MalformedMetadata)?;
        let name_end = name_start
            .checked_add(usize::from(entry.name_len))
            .ok_or(WimPathError::MalformedMetadata)?;
        if name_end > entry_end {
            return Err(WimPathError::MalformedMetadata);
        }

        if utf16le_name_eq_ascii(&metadata[name_start..name_end], name)? {
            return Ok(WimDirectoryEntryLocation { offset, entry });
        }

        offset = entry_end;
    }
}

fn parse_directory_entry(
    metadata: &[u8],
    offset: usize,
) -> Result<WimDirectoryEntry, WimPathError> {
    let mut hash = [0u8; WIM_HASH_SIZE];
    let hash_end = offset
        .checked_add(DIRECTORY_ENTRY_HASH_OFFSET + WIM_HASH_SIZE)
        .ok_or(WimPathError::MalformedMetadata)?;
    hash.copy_from_slice(
        metadata
            .get(offset + DIRECTORY_ENTRY_HASH_OFFSET..hash_end)
            .ok_or(WimPathError::MalformedMetadata)?,
    );

    Ok(WimDirectoryEntry {
        len: read_le_u64(metadata, offset).ok_or(WimPathError::MalformedMetadata)?,
        attributes: read_le_u32(metadata, offset + DIRECTORY_ENTRY_ATTRIBUTES_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        security: read_le_u32(metadata, offset + DIRECTORY_ENTRY_SECURITY_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        subdir: read_le_u64(metadata, offset + DIRECTORY_ENTRY_SUBDIR_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        hash,
        streams: read_le_u16(metadata, offset + DIRECTORY_ENTRY_STREAMS_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        short_name_len: read_le_u16(metadata, offset + DIRECTORY_ENTRY_SHORT_NAME_LEN_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        name_len: read_le_u16(metadata, offset + DIRECTORY_ENTRY_NAME_LEN_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
    })
}

fn utf16le_name_eq_ascii(name_bytes: &[u8], ascii: &str) -> Result<bool, WimPathError> {
    if ascii.bytes().any(|byte| byte >= 0x80) {
        return Err(WimPathError::NonAsciiPath);
    }
    if name_bytes.len() % 2 != 0 {
        return Err(WimPathError::MalformedMetadata);
    }

    let mut units = name_bytes.len() / 2;
    let expected = ascii.len();
    if units == expected + 1
        && read_le_u16(name_bytes, expected * 2).ok_or(WimPathError::MalformedMetadata)? == 0
    {
        units -= 1;
    }
    if units != expected {
        return Ok(false);
    }

    for (index, expected) in ascii.bytes().enumerate() {
        let actual = read_le_u16(name_bytes, index * 2).ok_or(WimPathError::MalformedMetadata)?;
        if actual > 0x7f {
            return Ok(false);
        }
        if !ascii_eq_ignore_case(actual as u8, expected) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn ascii_eq_ignore_case(left: u8, right: u8) -> bool {
    left.eq_ignore_ascii_case(&right)
}

fn align_up_8(value: usize) -> Option<usize> {
    value.checked_add(7).map(|value| value & !7)
}
