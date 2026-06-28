use super::model::IsoFile;
use super::IsoScanner;
use crate::source_disk::SourceDiskIdentity;
use crate::ventoy_config::VentoyConfig;
use nextboot_fs::btrfs::Btrfs;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::ext4::Ext4;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::xfs::Xfs;
use nextboot_fs::{FileSystem, FileSystemType};
use uefi::proto::media::block::BlockIO;
use uefi::Handle;

struct VlnkTarget<'a> {
    asset_volume_handle: Handle,
    asset_volume_index: usize,
    asset_source_disk: Option<SourceDiskIdentity>,
    asset_source_disk_size: u64,
    target_volume_handle: Handle,
    target_source_disk: Option<SourceDiskIdentity>,
    target_source_disk_size: u64,
    target_block_io: &'a BlockIO,
    target_path: &'a str,
    link_path: &'a str,
    config: &'a VentoyConfig,
    extent_lba_offset: u64,
}

impl<'a> IsoScanner<'a> {
    pub(super) fn resolve_vlnk_on_detected_fs(
        &self,
        asset_volume_handle: Handle,
        asset_volume_index: usize,
        asset_source_disk: Option<SourceDiskIdentity>,
        asset_source_disk_size: u64,
        target_volume_handle: Handle,
        target_source_disk: Option<SourceDiskIdentity>,
        target_source_disk_size: u64,
        target_block_io: &BlockIO,
        shared: nextboot_fs::SharedBlockIo,
        fs_type: FileSystemType,
        target_path: &str,
        link_path: &str,
        config: &VentoyConfig,
        extent_lba_offset: u64,
    ) -> Option<IsoFile> {
        let target = VlnkTarget {
            asset_volume_handle,
            asset_volume_index,
            asset_source_disk,
            asset_source_disk_size,
            target_volume_handle,
            target_source_disk,
            target_source_disk_size,
            target_block_io,
            target_path,
            link_path,
            config,
            extent_lba_offset,
        };

        match fs_type {
            FileSystemType::Fat32 => Fat32::open(shared)
                .ok()
                .and_then(|fs| self.build_vlnk_from_target_fs(&target, &fs)),
            FileSystemType::ExFat => ExFat::open(shared)
                .ok()
                .and_then(|fs| self.build_vlnk_from_target_fs(&target, &fs)),
            FileSystemType::Ntfs => Ntfs::open(shared)
                .ok()
                .and_then(|fs| self.build_vlnk_from_target_fs(&target, &fs)),
            FileSystemType::Xfs => Xfs::open(shared.clone())
                .ok()
                .and_then(|fs| self.build_vlnk_from_target_fs(&target, &fs))
                .or_else(|| self.probe_vlnk_fallback_filesystems(shared, &target)),
            FileSystemType::Btrfs => Btrfs::open(shared.clone())
                .ok()
                .and_then(|fs| self.build_vlnk_from_target_fs(&target, &fs))
                .or_else(|| self.probe_vlnk_fallback_filesystems(shared, &target)),
            _ => self.probe_vlnk_fallback_filesystems(shared, &target),
        }
    }

    fn probe_vlnk_fallback_filesystems(
        &self,
        shared: nextboot_fs::SharedBlockIo,
        target: &VlnkTarget,
    ) -> Option<IsoFile> {
        Udf::open(shared.clone())
            .ok()
            .and_then(|fs| self.build_vlnk_from_target_fs(target, &fs))
            .or_else(|| {
                Ext4::open(shared.clone())
                    .ok()
                    .and_then(|fs| self.build_vlnk_from_target_fs(target, &fs))
            })
            .or_else(|| {
                Xfs::open(shared.clone())
                    .ok()
                    .and_then(|fs| self.build_vlnk_from_target_fs(target, &fs))
            })
            .or_else(|| {
                Btrfs::open(shared.clone())
                    .ok()
                    .and_then(|fs| self.build_vlnk_from_target_fs(target, &fs))
            })
            .or_else(|| {
                Iso9660::open(shared)
                    .ok()
                    .and_then(|fs| self.build_vlnk_from_target_fs(target, &fs))
            })
    }

    fn build_vlnk_from_target_fs<F: FileSystem>(
        &self,
        target: &VlnkTarget,
        fs: &F,
    ) -> Option<IsoFile> {
        self.build_vlnk_iso_file_from_fs(
            target.asset_volume_handle,
            target.asset_volume_index,
            target.asset_source_disk,
            target.asset_source_disk_size,
            target.target_volume_handle,
            target.target_source_disk,
            target.target_source_disk_size,
            target.target_block_io,
            fs,
            target.target_path,
            target.link_path,
            target.config,
            target.extent_lba_offset,
        )
    }
}
