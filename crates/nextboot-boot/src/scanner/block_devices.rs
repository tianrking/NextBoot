use super::block_io::{alloc_buffer_for_block, UefiBlockIo};
use super::model::IsoFile;
use super::{block_io_info, handle_list_contains, IsoScanner};
use alloc::rc::Rc;
use alloc::vec::Vec;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::xfs::Xfs;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileSystemType};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::SearchType;
use uefi::{Handle, Identify};

mod partitioned;

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

            // Several real UEFI implementations expose the FAT ESP through
            // SimpleFileSystem but leave the exFAT data partition as raw
            // BlockIO.  Once that happens, scanning every BlockIO device also
            // walks the host's internal SSDs.  Keep the raw recovery path on
            // the physical disk that loaded this EFI application.
            if let Some(expected) = self.preferred_source_disk {
                let Some(candidate) = self.resolve_source_disk_identity(handle) else {
                    continue;
                };
                if !same_physical_disk(expected, candidate) {
                    continue;
                }
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
                FileSystemType::Xfs => {
                    let fs = match Xfs::open(shared) {
                        Ok(fs) => fs,
                        Err(err) => {
                            log::warn!("Ignoring XFS BlockIO volume {:?}: {:?}", handle, err);
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
}

fn same_physical_disk(
    left: crate::source_disk::SourceDiskIdentity,
    right: crate::source_disk::SourceDiskIdentity,
) -> bool {
    left.disk_guid == right.disk_guid
        && left.disk_signature == right.disk_signature
        && left.disk_size == right.disk_size
        && left.block_size == right.block_size
}

#[cfg(test)]
mod tests {
    use super::same_physical_disk;
    use crate::source_disk::{PartitionFormat, SourceDiskIdentity};

    fn disk(partition_number: u16, disk_size: u64) -> SourceDiskIdentity {
        SourceDiskIdentity {
            disk_guid: [7; 16],
            disk_signature: [3; 4],
            disk_size,
            block_size: 512,
            partition_number,
            partition_start_lba: u64::from(partition_number) * 2048,
            partition_size_blocks: 4096,
            partition_format: PartitionFormat::Gpt,
        }
    }

    #[test]
    fn compares_parent_disk_not_loaded_partition() {
        assert!(same_physical_disk(disk(1, 128 * 1024), disk(2, 128 * 1024)));
        assert!(!same_physical_disk(
            disk(1, 128 * 1024),
            disk(1, 256 * 1024)
        ));
    }
}
