use super::model::PartitionRange;
use super::IsoScanner;
use crate::source_disk::{parent_device_path_bytes, parse_last_hard_drive_device_path};
use alloc::vec::Vec;
use uefi::Handle;

impl<'a> IsoScanner<'a> {
    pub(super) fn exposed_child_partitions(
        &self,
        physical_handle: Handle,
        all_block_handles: &[Handle],
    ) -> Vec<PartitionRange> {
        let Some(physical_path) = self.handle_device_path_bytes(physical_handle) else {
            return Vec::new();
        };
        let mut ranges = Vec::new();

        for handle in all_block_handles.iter().copied() {
            if handle.as_ptr() == physical_handle.as_ptr() {
                continue;
            }
            let Some(path) = self.handle_device_path_bytes(handle) else {
                continue;
            };
            let Some(hard_drive) = parse_last_hard_drive_device_path(&path) else {
                continue;
            };
            let Some(parent_path) = parent_device_path_bytes(&path, &hard_drive) else {
                continue;
            };
            if parent_path != physical_path {
                continue;
            }
            if ranges.try_reserve_exact(1).is_err() {
                break;
            }
            ranges.push(PartitionRange {
                start_lba: hard_drive.partition_start_lba,
                block_count: hard_drive.partition_size_blocks,
            });
        }

        ranges
    }
}
