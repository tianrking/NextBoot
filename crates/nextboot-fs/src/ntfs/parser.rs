use super::*;

pub(super) fn parse_attribute(
    attr_type: u32,
    data: &[u8],
) -> Result<Option<NtfsAttribute>, FsError> {
    if data.len() < 16 {
        return Err(FsError::Corrupted);
    }

    let non_resident = data[8] != 0;
    let name_len = data[9];
    let attr_flags = read_u16(data, 12)?;
    if attr_flags & 0x4001 != 0 {
        return Err(FsError::UnsupportedFs);
    }

    if non_resident {
        if data.len() < 64 {
            return Err(FsError::Corrupted);
        }

        let lowest_vcn = read_u64(data, 16)?;
        let runlist_offset = read_u16(data, 32)? as usize;
        let real_size = read_u64(data, 48)?;
        if runlist_offset >= data.len() {
            return Err(FsError::Corrupted);
        }
        let runs = parse_data_runs(&data[runlist_offset..], lowest_vcn)?;
        Ok(Some(NtfsAttribute {
            attr_type,
            name_len,
            lowest_vcn,
            data: AttributeData::NonResident { real_size, runs },
        }))
    } else {
        if data.len() < 24 {
            return Err(FsError::Corrupted);
        }

        let value_len = read_u32(data, 16)? as usize;
        let value_offset = read_u16(data, 20)? as usize;
        let end = value_offset
            .checked_add(value_len)
            .ok_or(FsError::Corrupted)?;
        if end > data.len() {
            return Err(FsError::Corrupted);
        }

        let mut value = Vec::new();
        value
            .try_reserve_exact(value_len)
            .map_err(|_| FsError::OutOfMemory)?;
        value.extend_from_slice(&data[value_offset..end]);
        Ok(Some(NtfsAttribute {
            attr_type,
            name_len,
            lowest_vcn: 0,
            data: AttributeData::Resident { value },
        }))
    }
}

pub(super) fn parse_attribute_list_entries(
    data: &[u8],
    out: &mut Vec<AttributeListEntry>,
) -> Result<(), FsError> {
    let mut offset = 0usize;
    while offset + 26 <= data.len() {
        let attr_type = read_u32(data, offset)?;
        let entry_len = read_u16(data, offset + 4)? as usize;
        if attr_type == 0 || entry_len == 0 {
            break;
        }
        if entry_len < 26
            || offset
                .checked_add(entry_len)
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        let record_number = read_file_reference(data, offset + 16)?;
        out.try_reserve_exact(1).map_err(|_| FsError::OutOfMemory)?;
        out.push(AttributeListEntry {
            attr_type,
            record_number,
        });

        offset += entry_len;
    }

    Ok(())
}

fn parse_data_runs(data: &[u8], lowest_vcn: u64) -> Result<Vec<DataRun>, FsError> {
    let mut runs = Vec::new();
    let mut offset = 0usize;
    let mut current_vcn = lowest_vcn;
    let mut current_lcn = 0i64;

    while offset < data.len() {
        let header = data[offset];
        offset += 1;
        if header == 0 {
            break;
        }

        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        if len_size == 0
            || len_size > 8
            || off_size > 8
            || offset
                .checked_add(len_size)
                .and_then(|value| value.checked_add(off_size))
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        let cluster_count = read_le_uint(&data[offset..offset + len_size]);
        offset += len_size;
        if cluster_count == 0 {
            return Err(FsError::Corrupted);
        }

        let logical_cluster_start = if off_size == 0 {
            None
        } else {
            let delta = read_le_int(&data[offset..offset + off_size]);
            offset += off_size;
            current_lcn = current_lcn.checked_add(delta).ok_or(FsError::Corrupted)?;
            if current_lcn < 0 {
                return Err(FsError::Corrupted);
            }
            Some(current_lcn as u64)
        };

        runs.try_reserve_exact(1)
            .map_err(|_| FsError::OutOfMemory)?;
        runs.push(DataRun {
            virtual_cluster_start: current_vcn,
            logical_cluster_start,
            cluster_count,
        });
        current_vcn = current_vcn
            .checked_add(cluster_count)
            .ok_or(FsError::Corrupted)?;
    }

    Ok(runs)
}

pub(super) fn parse_index_entries(data: &[u8], entries: &mut Vec<FileInfo>) -> Result<(), FsError> {
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        let entry_len = read_u16(data, offset + 8)? as usize;
        let stream_len = read_u16(data, offset + 10)? as usize;
        let flags = read_u16(data, offset + 12)?;
        if entry_len == 0
            || offset
                .checked_add(entry_len)
                .map_or(true, |end| end > data.len())
        {
            return Err(FsError::Corrupted);
        }

        if flags & INDEX_ENTRY_LAST != 0 {
            break;
        }
        let stream_start = offset + 16;
        let stream_end = stream_start
            .checked_add(stream_len)
            .ok_or(FsError::Corrupted)?;
        if stream_end > offset + entry_len {
            return Err(FsError::Corrupted);
        }

        if let Some(info) =
            parse_file_name_entry(read_u48(data, offset)?, &data[stream_start..stream_end])?
        {
            entries
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            entries.push(info);
        }

        if flags & INDEX_ENTRY_HAS_CHILD != 0 {
            // Child VCN is stored in the entry tail; we only need the flattened
            // entries present in root/index allocation records for read-only use.
        }
        offset += entry_len;
    }

    Ok(())
}

fn parse_file_name_entry(record_number: u64, data: &[u8]) -> Result<Option<FileInfo>, FsError> {
    if data.len() < 66 {
        return Ok(None);
    }

    let namespace = data[65];
    if namespace == FILE_NAME_NAMESPACE_DOS {
        return Ok(None);
    }

    let allocated_size = read_u64(data, 40)?;
    let real_size = read_u64(data, 48)?;
    let raw_flags = read_u32(data, 56)?;
    let name_len = data[64] as usize;
    let name_bytes = name_len.checked_mul(2).ok_or(FsError::Corrupted)?;
    let name_start = 66usize;
    let name_end = name_start
        .checked_add(name_bytes)
        .ok_or(FsError::Corrupted)?;
    if name_end > data.len() {
        return Err(FsError::Corrupted);
    }

    let name = utf16le_to_string(&data[name_start..name_end])?;
    if name.is_empty() || name == "." || name == ".." {
        return Ok(None);
    }

    let is_dir = raw_flags & FILE_ATTRIBUTE_DIRECTORY != 0;
    let mut attributes = FileAttributes::empty();
    if raw_flags & FILE_ATTRIBUTE_READ_ONLY != 0 {
        attributes |= FileAttributes::READ_ONLY;
    }
    if raw_flags & FILE_ATTRIBUTE_HIDDEN != 0 {
        attributes |= FileAttributes::HIDDEN;
    }
    if raw_flags & FILE_ATTRIBUTE_SYSTEM != 0 {
        attributes |= FileAttributes::SYSTEM;
    }
    if raw_flags & FILE_ATTRIBUTE_ARCHIVE != 0 {
        attributes |= FileAttributes::ARCHIVE;
    }
    if is_dir {
        attributes |= FileAttributes::DIRECTORY;
    }

    Ok(Some(FileInfo {
        name,
        size: if is_dir { allocated_size } else { real_size },
        is_dir,
        attributes,
        start_cluster: record_number,
        contiguous: false,
    }))
}

fn utf16le_to_string(data: &[u8]) -> Result<String, FsError> {
    if data.len() % 2 != 0 {
        return Err(FsError::Corrupted);
    }

    let mut out = String::new();
    for chunk in data.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        if value == 0 {
            break;
        }
        let Some(ch) = char::from_u32(value as u32) else {
            return Err(FsError::Corrupted);
        };
        out.try_reserve(ch.len_utf8())
            .map_err(|_| FsError::OutOfMemory)?;
        out.push(ch);
    }
    Ok(out)
}
