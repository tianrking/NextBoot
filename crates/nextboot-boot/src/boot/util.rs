use crate::scanner::{IsoFile, OsType};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;
use nextboot_virtio::VirtualDeviceType;
use uefi::proto::device_path::DevicePath;

pub(super) fn selected_ventoy_plugin_index(autosel: Option<usize>, count: usize) -> Option<usize> {
    match autosel {
        Some(0) | None => None,
        Some(value) if value <= count => Some(value - 1),
        _ => None,
    }
}

pub(super) fn selected_persistence_backend_index(
    autosel: Option<usize>,
    count: usize,
) -> Option<usize> {
    match autosel {
        Some(0) => None,
        Some(value) => selected_ventoy_plugin_index(Some(value), count),
        None if count == 1 => Some(0),
        None => None,
    }
}

pub(super) fn iso9660_file_extent_patch(first_sector: u32, size: u32) -> Vec<u8> {
    let mut patch = Vec::new();
    patch.extend_from_slice(&first_sector.to_le_bytes());
    patch.extend_from_slice(&first_sector.to_be_bytes());
    patch.extend_from_slice(&size.to_le_bytes());
    patch.extend_from_slice(&size.to_be_bytes());
    patch
}

pub(super) fn normalize_iso_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = trimmed.trim_start_matches(['/', '\\']);
    let mut normalized = String::from("/");
    let mut first = true;

    for part in trimmed
        .split(|ch| ch == '/' || ch == '\\')
        .filter(|part| !part.is_empty())
    {
        if !first {
            normalized.push('/');
        }
        normalized.push_str(part);
        first = false;
    }

    normalized
}

pub(super) fn push_unique_iso_path(paths: &mut Vec<String>, path: &str) -> uefi::Result<()> {
    let normalized = normalize_iso_path(path);
    if paths.iter().any(|existing| existing == &normalized) {
        return Ok(());
    }

    paths
        .try_reserve_exact(1)
        .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
    paths.push(normalized);
    Ok(())
}

pub(super) fn is_isolinux_config_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/isolinux/") || lower.contains("/syslinux/") || lower.ends_with("isolinux.cfg")
}

pub(super) fn iso_parent_dir(path: &str) -> String {
    let normalized = normalize_iso_path(path);
    match normalized.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(index) => String::from(&normalized[..index]),
    }
}

pub(super) fn resolve_linux_config_path(base_dir: &str, path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return normalize_iso_path(trimmed);
    }

    if base_dir == "/" {
        normalize_iso_path(trimmed)
    } else {
        normalize_iso_path(&format!("{}/{}", base_dir, trimmed))
    }
}

pub(super) fn runtime_extent_count(iso: &IsoFile) -> usize {
    if iso.extents.is_empty() {
        1
    } else {
        iso.extents.len()
    }
}

pub(super) fn device_path_to_vec(device_path: &DevicePath) -> uefi::Result<Vec<u8>> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }.ok_or(uefi::Status::INVALID_PARAMETER)?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Ok(bytes.to_vec())
}

unsafe fn device_path_byte_len(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }

    let mut offset = 0usize;
    loop {
        let node = unsafe { ptr.add(offset) };
        let node_type = unsafe { ptr::read_unaligned(node) };
        let node_subtype = unsafe { ptr::read_unaligned(node.add(1)) };
        let len_lo = unsafe { ptr::read_unaligned(node.add(2)) };
        let len_hi = unsafe { ptr::read_unaligned(node.add(3)) };
        let node_len = u16::from_le_bytes([len_lo, len_hi]) as usize;
        if node_len < 4 {
            return None;
        }

        offset = offset.checked_add(node_len)?;
        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }
    }
}

pub(super) fn is_child_device_path(parent: &[u8], child: &[u8]) -> bool {
    let parent_prefix_len = parent_without_end_len(parent).unwrap_or(parent.len());
    child.len() >= parent_prefix_len
        && child.get(..parent_prefix_len) == parent.get(..parent_prefix_len)
}

fn parent_without_end_len(path: &[u8]) -> Option<usize> {
    if path.len() < 4 {
        return None;
    }

    let mut offset = 0usize;
    while offset.checked_add(4)? <= path.len() {
        let node_type = *path.get(offset)?;
        let node_subtype = *path.get(offset + 1)?;
        let node_len =
            u16::from_le_bytes([*path.get(offset + 2)?, *path.get(offset + 3)?]) as usize;
        if node_len < 4 || offset.checked_add(node_len)? > path.len() {
            return None;
        }

        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }

        offset += node_len;
    }

    None
}

pub(super) fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

pub(super) fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }

    value.checked_add(divisor - 1).map(|value| value / divisor)
}

pub(super) fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    if align == 0 {
        return None;
    }

    let remainder = value % align;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(align - remainder)
    }
}

pub(super) fn usize_to_u16(value: usize) -> uefi::Result<u16> {
    u16::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

pub(super) fn usize_to_u32(value: usize) -> uefi::Result<u32> {
    u32::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

pub(super) fn os_type_code(os_type: OsType) -> u32 {
    match os_type {
        OsType::Unknown => 0,
        OsType::Windows => 1,
        OsType::WinPE => 2,
        OsType::Linux => 10,
        OsType::Ubuntu => 11,
        OsType::Debian => 12,
        OsType::Fedora => 13,
        OsType::Arch => 14,
    }
}

pub(super) fn ventoy_chain_type(iso: &IsoFile) -> u8 {
    if iso.image_format.is_wim_container() {
        return crate::ventoy::VENTOY_CHAIN_WIM;
    }

    match iso.os_type {
        OsType::Windows | OsType::WinPE => crate::ventoy::VENTOY_CHAIN_WINDOWS,
        _ => crate::ventoy::VENTOY_CHAIN_LINUX,
    }
}

pub(super) fn virtual_device_type_code(device_type: VirtualDeviceType) -> u32 {
    match device_type {
        VirtualDeviceType::DvdRom => 1,
        VirtualDeviceType::HardDisk => 2,
        VirtualDeviceType::UsbMassStorage => 3,
    }
}

pub(super) fn push_u16(data: &mut Vec<u8>, value: u16) {
    data.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_extent_record(
    data: &mut Vec<u8>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    push_u64(data, virtual_block_start);
    push_u64(data, physical_lba);
    push_u64(data, block_count);
}
