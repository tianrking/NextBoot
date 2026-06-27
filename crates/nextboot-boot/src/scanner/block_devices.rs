use super::block_io::{alloc_buffer_for_block, PartitionBlockIo, UefiBlockIo};
use super::model::{IsoFile, PartitionRange};
use super::partitions::discover_partition_candidates;
use super::{block_io_info, handle_list_contains, partition_source_disk_identity, IsoScanner};
use crate::source_disk::{
    parent_device_path_bytes, parse_last_hard_drive_device_path, SourceDiskIdentity,
};
use alloc::rc::Rc;
use alloc::vec::Vec;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileSystemType};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::SearchType;
use uefi::{Handle, Identify};

impl<'a> IsoScanner<'a> {
    pub(super) fn scan_block_filesystem_volumes(
        &self,
        volume_index_base: usize,
        simple_fs_handles: &[Handle],
        default_search_paths: &[&str],
        extensions: &[&str],
    ) -> uefi::Result<Vec<IsoFile>> {
        let block_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))?;
        let mut files = Vec::new();
        let mut block_volume_index = 0usize;
        let all_block_handles: Vec<Handle> = block_handles.iter().copied().collect();

        for handle in all_block_handles.iter().copied() {
            if handle_list_contains(simple_fs_handles, handle) {
                continue;
            }

            let block_io = match self.bt.open_protocol_exclusive::<BlockIO>(handle) {
                Ok(block_io) => block_io,
                Err(_) => continue,
            };
            let media = block_io.media();
            if !media.is_media_present() {
                continue;
            }
            let block_size = media.block_size();
            if block_size == 0 {
                continue;
            }

            let Some(uefi_io) = UefiBlockIo::new(&block_io) else {
                continue;
            };
            let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
            let mut boot_sector = match alloc_buffer_for_block(block_size) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
            if shared.read_blocks(0, &mut boot_sector).is_err() {
                continue;
            }

            let fs_type = detect_fs_type(&boot_sector);
            if !matches!(
                fs_type,
                FileSystemType::Fat32 | FileSystemType::ExFat | FileSystemType::Ntfs
            ) {
                let scanned = self.scan_partitioned_block_device(
                    handle,
                    &all_block_handles,
                    volume_index_base,
                    &mut block_volume_index,
                    &block_io,
                    shared.clone(),
                    &boot_sector,
                    default_search_paths,
                    extensions,
                    &mut files,
                );
                if scanned > 0 {
                    continue;
                }

                let volume_index = volume_index_base + block_volume_index;
                let source_disk = self.resolve_source_disk_identity(handle);
                let source_disk_size = source_disk
                    .map(|disk| disk.disk_size)
                    .or_else(|| block_io_info(&block_io).map(|info| info.total_size))
                    .unwrap_or(0);
                if self.scan_unknown_block_filesystem_volume(
                    handle,
                    volume_index,
                    source_disk,
                    source_disk_size,
                    &block_io,
                    shared,
                    default_search_paths,
                    extensions,
                    0,
                    &mut files,
                ) {
                    block_volume_index += 1;
                }
                continue;
            }

            let volume_index = volume_index_base + block_volume_index;
            let source_disk = self.resolve_source_disk_identity(handle);
            let source_disk_size = source_disk
                .map(|disk| disk.disk_size)
                .or_else(|| block_io_info(&block_io).map(|info| info.total_size))
                .unwrap_or(0);

            match fs_type {
                FileSystemType::Fat32 => {
                    let fs = match Fat32::open(shared.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring FAT32 BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                FileSystemType::ExFat => {
                    let fs = match ExFat::open(shared.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring exFAT BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                FileSystemType::Ntfs => {
                    let fs = match Ntfs::open(shared) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring NTFS BlockIO volume {:?}: {:?}", handle, err);
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        &block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        0,
                        &mut files,
                    );
                }
                _ => {}
            }
            block_volume_index += 1;
        }

        Ok(files)
    }

    fn scan_unknown_block_filesystem_volume(
        &self,
        volume_handle: Handle,
        volume_index: usize,
        source_disk: Option<SourceDiskIdentity>,
        source_disk_size: u64,
        block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        default_search_paths: &[&str],
        extensions: &[&str],
        extent_lba_offset: u64,
        files: &mut Vec<IsoFile>,
    ) -> bool {
        if let Ok(fs) = Udf::open(shared.clone()) {
            self.scan_block_filesystem_paths(
                volume_handle,
                volume_index,
                source_disk,
                source_disk_size,
                block_io,
                &fs,
                default_search_paths,
                extensions,
                extent_lba_offset,
                files,
            );
            return true;
        }

        if let Ok(fs) = Iso9660::open(shared) {
            self.scan_block_filesystem_paths(
                volume_handle,
                volume_index,
                source_disk,
                source_disk_size,
                block_io,
                &fs,
                default_search_paths,
                extensions,
                extent_lba_offset,
                files,
            );
            return true;
        }

        false
    }

    fn scan_partitioned_block_device(
        &self,
        physical_handle: Handle,
        all_block_handles: &[Handle],
        volume_index_base: usize,
        block_volume_index: &mut usize,
        block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        first_block: &[u8],
        default_search_paths: &[&str],
        extensions: &[&str],
        files: &mut Vec<IsoFile>,
    ) -> usize {
        let Some(volume_info) = block_io_info(block_io) else {
            return 0;
        };
        let partitions = discover_partition_candidates(shared.clone(), first_block);
        if partitions.is_empty() {
            return 0;
        }

        let exposed = self.exposed_child_partitions(physical_handle, all_block_handles);
        let mut scanned = 0usize;
        for partition in partitions {
            if exposed
                .iter()
                .any(|range| range.matches(partition.start_lba, partition.block_count))
            {
                continue;
            }
            if partition.block_count == 0
                || partition
                    .start_lba
                    .checked_add(partition.block_count)
                    .map_or(true, |end| end > shared.total_blocks())
            {
                continue;
            }

            let partition_io: nextboot_fs::SharedBlockIo = Rc::new(PartitionBlockIo::new(
                shared.clone(),
                partition.start_lba,
                partition.block_count,
            ));
            let mut boot_sector = match alloc_buffer_for_block(partition_io.block_size()) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
            if partition_io.read_blocks(0, &mut boot_sector).is_err() {
                continue;
            }
            let fs_type = detect_fs_type(&boot_sector);

            let volume_index = volume_index_base + *block_volume_index;
            let source_disk = partition_source_disk_identity(first_block, volume_info, partition);
            let source_disk_size = source_disk
                .map(|disk| disk.disk_size)
                .unwrap_or(volume_info.total_size);

            match fs_type {
                FileSystemType::Fat32 => {
                    let fs = match Fat32::open(partition_io.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring FAT32 partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                FileSystemType::ExFat => {
                    let fs = match ExFat::open(partition_io.clone()) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring exFAT partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                FileSystemType::Ntfs => {
                    let fs = match Ntfs::open(partition_io) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!(
                                "Ignoring NTFS partition {} on {:?}: {:?}",
                                partition.number,
                                physical_handle,
                                err
                            );
                            continue;
                        }
                    };
                    self.scan_block_filesystem_paths(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        &fs,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    );
                }
                _ => {
                    if !self.scan_unknown_block_filesystem_volume(
                        physical_handle,
                        volume_index,
                        source_disk,
                        source_disk_size,
                        block_io,
                        partition_io,
                        default_search_paths,
                        extensions,
                        partition.start_lba,
                        files,
                    ) {
                        continue;
                    }
                }
            }

            *block_volume_index += 1;
            scanned += 1;
        }

        scanned
    }

    fn exposed_child_partitions(
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
