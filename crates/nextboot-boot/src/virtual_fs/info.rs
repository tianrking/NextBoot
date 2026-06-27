use super::*;

pub(super) fn build_file_info_bytes(info: &FsFileInfo, block_size: u32) -> Result<Vec<u8>, Status> {
    let mut name = encode_utf16_nul(&info.name)?;
    if name.is_empty() {
        name.push(0);
    }

    let name_bytes = name.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let unaligned_size = EFI_FILE_INFO_NAME_OFFSET
        .checked_add(name_bytes)
        .ok_or(Status::OUT_OF_RESOURCES)?;
    let total_size = align_up(unaligned_size, 8).ok_or(Status::OUT_OF_RESOURCES)?;
    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;

    push_u64(&mut data, total_size as u64);
    push_u64(&mut data, info.size);
    push_u64(&mut data, physical_size(info.size, block_size));
    append_efi_time(&mut data);
    append_efi_time(&mut data);
    append_efi_time(&mut data);
    push_u64(&mut data, efi_file_attributes(info));
    debug_assert_eq!(data.len(), EFI_FILE_INFO_NAME_OFFSET);
    append_utf16(&mut data, &name);
    data.resize(total_size, 0);
    Ok(data)
}

pub(super) fn build_file_system_info_bytes(
    volume_size: u64,
    block_size: u32,
) -> Result<Vec<u8>, Status> {
    let label = encode_utf16_nul(VOLUME_LABEL)?;
    let label_bytes = label.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let unaligned_size = EFI_FILE_SYSTEM_INFO_LABEL_OFFSET
        .checked_add(label_bytes)
        .ok_or(Status::OUT_OF_RESOURCES)?;
    let total_size = align_up(unaligned_size, 8).ok_or(Status::OUT_OF_RESOURCES)?;

    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    push_u64(&mut data, total_size as u64);
    data.push(1);
    data.extend_from_slice(&[0; 7]);
    push_u64(&mut data, volume_size);
    push_u64(&mut data, 0);
    push_u32(&mut data, block_size);
    debug_assert_eq!(data.len(), EFI_FILE_SYSTEM_INFO_LABEL_OFFSET);
    append_utf16(&mut data, &label);
    data.resize(total_size, 0);
    Ok(data)
}

pub(super) fn build_volume_label_bytes() -> Result<Vec<u8>, Status> {
    let label = encode_utf16_nul(VOLUME_LABEL)?;
    let total_size = label.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    append_utf16(&mut data, &label);
    Ok(data)
}

pub(super) unsafe fn copy_info_response(
    bytes: &[u8],
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    if buffer_size.is_null() {
        return Status::INVALID_PARAMETER;
    }

    let provided = unsafe { *buffer_size };
    unsafe {
        *buffer_size = bytes.len();
    }
    if buffer.is_null() || provided < bytes.len() {
        return Status::BUFFER_TOO_SMALL;
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
    }
    Status::SUCCESS
}

fn encode_utf16_nul(value: &str) -> Result<Vec<u16>, Status> {
    let mut out = Vec::new();
    out.try_reserve_exact(value.len().saturating_add(1))
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    for unit in value.encode_utf16() {
        out.push(unit);
    }
    out.push(0);
    Ok(out)
}

fn append_utf16(data: &mut Vec<u8>, units: &[u16]) {
    for unit in units {
        data.extend_from_slice(&unit.to_le_bytes());
    }
}

fn append_efi_time(data: &mut Vec<u8>) {
    data.extend_from_slice(&[0; 16]);
}

fn physical_size(file_size: u64, block_size: u32) -> u64 {
    let block_size = u64::from(block_size);
    if file_size == 0 || block_size == 0 {
        return file_size;
    }

    file_size
        .checked_add(block_size - 1)
        .map(|size| (size / block_size) * block_size)
        .unwrap_or(file_size)
}

fn efi_file_attributes(info: &FsFileInfo) -> u64 {
    let mut attrs = EFI_FILE_ATTR_READ_ONLY;
    if info.is_dir {
        attrs |= EFI_FILE_ATTR_DIRECTORY;
    } else {
        attrs |= EFI_FILE_ATTR_ARCHIVE;
    }
    if info.attributes.contains(FsFileAttributes::HIDDEN) {
        attrs |= EFI_FILE_ATTR_HIDDEN;
    }
    if info.attributes.contains(FsFileAttributes::SYSTEM) {
        attrs |= EFI_FILE_ATTR_SYSTEM;
    }
    attrs
}

pub(super) unsafe fn string_from_uefi_char16(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }

    let mut out = String::new();
    for index in 0..4096 {
        let unit = unsafe { ptr::read_unaligned(ptr.add(index)) };
        if unit == 0 {
            return Some(out);
        }

        let ch = char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}');
        out.push(if ch == '\\' { '/' } else { ch });
    }

    warn!("UEFI file path was not NUL terminated within 4096 UTF-16 code units");
    None
}

pub(super) fn resolve_child_path(base: &str, requested: &str) -> String {
    if requested.is_empty() || requested == "." {
        return normalize_path_segments(base);
    }

    if requested.starts_with('/') || requested.starts_with('\\') {
        return normalize_path_segments(requested);
    }

    let mut combined = String::new();
    combined.push_str(base.trim_end_matches(['/', '\\']));
    if combined.is_empty() {
        combined.push('/');
    }
    if !combined.ends_with('/') {
        combined.push('/');
    }
    combined.push_str(requested);
    normalize_path_segments(&combined)
}

pub(super) fn fs_error_to_status(err: FsError) -> Status {
    match err {
        FsError::FileNotFound | FsError::DirectoryNotFound => Status::NOT_FOUND,
        FsError::InvalidPath | FsError::InvalidArgument => Status::INVALID_PARAMETER,
        FsError::OutOfMemory | FsError::FileTooLarge => Status::OUT_OF_RESOURCES,
        FsError::NotDirectory | FsError::NotFile | FsError::UnsupportedFs => Status::UNSUPPORTED,
        FsError::InvalidSignature | FsError::BlockSizeMismatch | FsError::Corrupted => {
            Status::VOLUME_CORRUPTED
        }
        FsError::ReadError => Status::DEVICE_ERROR,
    }
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}
