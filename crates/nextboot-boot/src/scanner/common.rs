use super::model::{PartitionCandidate, VolumeBlockInfo};
use crate::source_disk::{
    build_source_disk_identity, HardDriveDevicePathInfo, PartitionFormat, SourceDiskIdentity,
};
use crate::vlnk::{self, VentoyVlnk};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;
use uefi::data_types::CString16;
use uefi::proto::device_path::DevicePath;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};
use uefi::{Handle, Status};

pub(super) fn block_io_info(block_io: &BlockIO) -> Option<VolumeBlockInfo> {
    let media = block_io.media();
    if !media.is_media_present() {
        return None;
    }

    let block_size = media.block_size();
    if block_size == 0 {
        return None;
    }
    let total_blocks = media.last_block().checked_add(1)?;
    let total_size = total_blocks.checked_mul(u64::from(block_size))?;

    Some(VolumeBlockInfo {
        block_size,
        total_size,
    })
}

pub(super) fn partition_source_disk_identity(
    first_block: &[u8],
    volume_info: VolumeBlockInfo,
    partition: PartitionCandidate,
) -> Option<SourceDiskIdentity> {
    let info = HardDriveDevicePathInfo {
        node_offset: 0,
        partition_number: partition.number,
        partition_start_lba: partition.start_lba,
        partition_size_blocks: partition.block_count,
        partition_format: partition.format,
        signature_type: match partition.format {
            PartitionFormat::Gpt => 2,
            PartitionFormat::Mbr => 1,
            PartitionFormat::Unknown => 0,
        },
    };
    build_source_disk_identity(
        first_block,
        volume_info.total_size,
        volume_info.block_size,
        Some(info),
    )
}

pub(super) fn read_uefi_regular_file(
    parent: &mut Directory,
    name: &str,
    expected_size: u64,
) -> uefi::Result<Vec<u8>> {
    if expected_size != vlnk::VLNK_FILE_LEN as u64 {
        return Err(Status::INVALID_PARAMETER.into());
    }
    let file_size = usize::try_from(expected_size).map_err(|_| Status::OUT_OF_RESOURCES)?;
    let c_path = CString16::try_from(name).map_err(|_| Status::INVALID_PARAMETER)?;
    let handle = parent.open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())?;
    let mut file = handle
        .into_regular_file()
        .ok_or_else(|| uefi::Error::new(Status::NOT_FOUND, ()))?;
    let mut data = Vec::new();
    data.try_reserve_exact(file_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    data.resize(file_size, 0);

    let mut offset = 0usize;
    while offset < data.len() {
        let read = file.read(&mut data[offset..])?;
        if read == 0 {
            break;
        }
        offset = offset
            .checked_add(read)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
    }
    data.truncate(offset);
    Ok(data)
}

pub(super) fn vlnk_matches_source_disk(
    source_disk: Option<SourceDiskIdentity>,
    vlnk: &VentoyVlnk,
) -> bool {
    let Some(disk) = source_disk else {
        return false;
    };
    if disk.disk_signature != vlnk.disk_signature {
        return false;
    }
    partition_offset_matches(
        disk.partition_start_lba,
        disk.block_size,
        vlnk.part_offset_bytes,
    )
}

pub(super) fn vlnk_matches_partition(
    partition: PartitionCandidate,
    block_size: u32,
    vlnk: &VentoyVlnk,
) -> bool {
    partition_offset_matches(partition.start_lba, block_size, vlnk.part_offset_bytes)
}

pub(super) fn normalize_vlnk_target_path(path: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for ch in path.trim().chars() {
        let ch = if ch == '\\' { '/' } else { ch };
        if ch == '/' {
            if previous_was_separator {
                continue;
            }
            previous_was_separator = true;
        } else {
            previous_was_separator = false;
        }
        normalized.push(ch);
    }
    if normalized.is_empty() {
        return String::from("/");
    }
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    normalized
}

pub(super) fn handle_list_contains(handles: &[Handle], needle: Handle) -> bool {
    handles
        .iter()
        .any(|handle| handle.as_ptr() == needle.as_ptr())
}

pub(super) fn should_descend_into_directory(depth: usize, max_search_level: Option<usize>) -> bool {
    max_search_level.map_or(true, |max_depth| depth < max_depth)
}

pub(super) fn device_path_to_vec(device_path: &DevicePath) -> Option<Vec<u8>> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Some(bytes.to_vec())
}

fn partition_offset_matches(start_lba: u64, block_size: u32, expected_bytes: u64) -> bool {
    let native = start_lba
        .checked_mul(u64::from(block_size))
        .is_some_and(|offset| offset == expected_bytes);
    let ventoy_sector = start_lba
        .checked_mul(512)
        .is_some_and(|offset| offset == expected_bytes);
    native || ventoy_sector
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
        if node_type == 0x7f && node_subtype == 0xff {
            return Some(offset);
        }
    }
}
