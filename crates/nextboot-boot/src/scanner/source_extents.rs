use alloc::vec::Vec;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::ext4::Ext4;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::xfs::Xfs;
use nextboot_fs::{FileExtent, FileSystem, FileSystemType, SharedBlockIo};

pub(super) fn source_file_extents_from_detected_fs(
    shared: SharedBlockIo,
    fs_type: FileSystemType,
    path: &str,
) -> Option<(u32, Vec<FileExtent>)> {
    match fs_type {
        FileSystemType::Fat32 => Fat32::open(shared)
            .and_then(|fs| extents_from_fs(fs, path))
            .ok(),
        FileSystemType::ExFat => ExFat::open(shared)
            .and_then(|fs| extents_from_fs(fs, path))
            .ok(),
        FileSystemType::Ntfs => Ntfs::open(shared)
            .and_then(|fs| extents_from_fs(fs, path))
            .ok(),
        FileSystemType::Xfs => Xfs::open(shared)
            .and_then(|fs| extents_from_fs(fs, path))
            .ok(),
        _ => Udf::open(shared.clone())
            .and_then(|fs| extents_from_fs(fs, path))
            .or_else(|_| Ext4::open(shared.clone()).and_then(|fs| extents_from_fs(fs, path)))
            .or_else(|_| Iso9660::open(shared).and_then(|fs| extents_from_fs(fs, path)))
            .ok(),
    }
}

fn extents_from_fs<F: FileSystem>(
    fs: F,
    path: &str,
) -> Result<(u32, Vec<FileExtent>), nextboot_fs::FsError> {
    let block_size = fs.block_size();
    fs.file_extents(path).map(|extents| (block_size, extents))
}
