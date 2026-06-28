use super::errors::{fs_error_to_uefi_status, virtio_error_to_fs_error};
use super::util::normalize_iso_path;
use super::wimboot_runtime::WimbootMappedSegment;
use crate::scanner::IsoExtent;
use crate::source_disk::{source_volume_range, SourceDiskIdentity};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::NonNull;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::ext4::Ext4;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::ntfs::Ntfs;
use nextboot_fs::udf::Udf;
use nextboot_fs::xfs::Xfs;
use nextboot_fs::{
    detect_fs_type, BlockIoOps, FileExtent, FileSystem, FileSystemType, FsError, SharedBlockIo,
};
use nextboot_virtio::{PhysicalReader, VirtIoError};
use uefi::proto::media::block::BlockIO;
use uefi::Status;

struct UefiPhysicalReader {
    block_io: NonNull<BlockIO>,
    media_id: u32,
    block_size: u32,
    total_blocks: u64,
}

impl UefiPhysicalReader {
    fn new(block_io: &BlockIO) -> Option<Self> {
        let media = block_io.media();
        let block_size = media.block_size();
        if block_size == 0 || !media.is_media_present() {
            return None;
        }

        Some(Self {
            block_io: NonNull::from(block_io),
            media_id: media.media_id(),
            block_size,
            total_blocks: media.last_block() + 1,
        })
    }
}

impl PhysicalReader for UefiPhysicalReader {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(VirtIoError::OutOfBounds);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|err| match err.status() {
                Status::MEDIA_CHANGED => VirtIoError::MediaChanged,
                Status::NO_MEDIA => VirtIoError::NoPhysicalRead,
                Status::BAD_BUFFER_SIZE => VirtIoError::InvalidBufferSize,
                Status::INVALID_PARAMETER => VirtIoError::InvalidArgument,
                _ => VirtIoError::ReadFailed,
            })
    }
}

impl BlockIoOps for UefiPhysicalReader {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        PhysicalReader::read_blocks(self, lba, buf).map_err(virtio_error_to_fs_error)
    }
}

pub(super) struct ZeroPhysicalReader;

impl PhysicalReader for ZeroPhysicalReader {
    fn read_blocks(&self, _lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        buf.fill(0);
        Ok(())
    }
}

pub(super) struct SourceVolumeReader {
    base: UefiPhysicalReader,
    lba_offset: u64,
    total_blocks: u64,
}

impl SourceVolumeReader {
    pub(super) fn new(block_io: &BlockIO, source_disk: Option<SourceDiskIdentity>) -> Option<Self> {
        let base = UefiPhysicalReader::new(block_io)?;
        let (lba_offset, total_blocks) = source_volume_range(base.total_blocks, source_disk)?;

        Some(Self {
            base,
            lba_offset,
            total_blocks,
        })
    }
}

impl PhysicalReader for SourceVolumeReader {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        let block_size = self.block_size() as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(VirtIoError::OutOfBounds);
        }

        let physical_lba = self
            .lba_offset
            .checked_add(lba)
            .ok_or(VirtIoError::OutOfBounds)?;
        PhysicalReader::read_blocks(&self.base, physical_lba, buf)
    }
}

impl BlockIoOps for SourceVolumeReader {
    fn block_size(&self) -> u32 {
        self.base.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        PhysicalReader::read_blocks(self, lba, buf).map_err(virtio_error_to_fs_error)
    }
}

pub(super) struct SourceVolumeFile {
    pub(super) path: String,
    pub(super) data: Vec<u8>,
}

pub(super) struct IsoMappedFileMetadata {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) segments: Vec<WimbootMappedSegment>,
}

pub(super) struct SourceVolumeFileMetadata {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) block_size: u32,
    pub(super) extents: Vec<IsoExtent>,
}

pub(super) enum SourceVolumeFileSystem {
    Fat32(Fat32),
    ExFat(ExFat),
    Ext4(Ext4),
    Ntfs(Ntfs),
    Udf(Udf),
    Xfs(Xfs),
    Iso9660(Iso9660),
}

impl SourceVolumeFileSystem {
    pub(super) fn open(
        block_io: &BlockIO,
        source_disk: Option<SourceDiskIdentity>,
    ) -> uefi::Result<Self> {
        let reader =
            SourceVolumeReader::new(block_io, source_disk).ok_or(uefi::Status::DEVICE_ERROR)?;
        let shared: SharedBlockIo = Rc::new(reader);
        let block_size =
            usize::try_from(shared.block_size()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        if block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let mut boot_sector = Vec::new();
        boot_sector
            .try_reserve_exact(block_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        boot_sector.resize(block_size, 0);
        shared
            .read_blocks(0, &mut boot_sector)
            .map_err(fs_error_to_uefi_status)?;

        match detect_fs_type(&boot_sector) {
            FileSystemType::Fat32 => Ok(Fat32::open(shared)
                .map(Self::Fat32)
                .map_err(fs_error_to_uefi_status)?),
            FileSystemType::ExFat => Ok(ExFat::open(shared)
                .map(Self::ExFat)
                .map_err(fs_error_to_uefi_status)?),
            FileSystemType::Ntfs => Ok(Ntfs::open(shared)
                .map(Self::Ntfs)
                .map_err(fs_error_to_uefi_status)?),
            FileSystemType::Xfs => Ok(Xfs::open(shared)
                .map(Self::Xfs)
                .map_err(fs_error_to_uefi_status)?),
            _ => Ok(Udf::open(shared.clone())
                .map(Self::Udf)
                .or_else(|_| Ext4::open(shared.clone()).map(Self::Ext4))
                .or_else(|_| Iso9660::open(shared).map(Self::Iso9660))
                .map_err(fs_error_to_uefi_status)?),
        }
    }

    pub(super) fn load_file(&self, path: &str) -> uefi::Result<SourceVolumeFile> {
        let metadata = self.file_metadata(path)?;
        let file_size =
            usize::try_from(metadata.size).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.resize(file_size, 0);

        let read = self.read_file(&metadata.path, 0, &mut data)?;
        data.truncate(read);

        Ok(SourceVolumeFile {
            path: metadata.path.clone(),
            data,
        })
    }

    pub(super) fn file_metadata(&self, path: &str) -> uefi::Result<SourceVolumeFileMetadata> {
        let path = normalize_iso_path(path);
        let info = self.stat(&path)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let extents = self
            .file_extents(&path)?
            .into_iter()
            .map(IsoExtent::from)
            .collect();

        Ok(SourceVolumeFileMetadata {
            path,
            size: info.size,
            block_size: self.block_size(),
            extents,
        })
    }

    pub(super) fn stat(&self, path: &str) -> uefi::Result<nextboot_fs::FileInfo> {
        Ok(match self {
            Self::Fat32(fs) => fs.stat(path),
            Self::ExFat(fs) => fs.stat(path),
            Self::Ext4(fs) => fs.stat(path),
            Self::Ntfs(fs) => fs.stat(path),
            Self::Udf(fs) => fs.stat(path),
            Self::Xfs(fs) => fs.stat(path),
            Self::Iso9660(fs) => fs.stat(path),
        }
        .map_err(fs_error_to_uefi_status)?)
    }

    pub(super) fn read_dir(&self, path: &str) -> uefi::Result<Vec<nextboot_fs::FileInfo>> {
        Ok(match self {
            Self::Fat32(fs) => fs.read_dir(path),
            Self::ExFat(fs) => fs.read_dir(path),
            Self::Ext4(fs) => fs.read_dir(path),
            Self::Ntfs(fs) => fs.read_dir(path),
            Self::Udf(fs) => fs.read_dir(path),
            Self::Xfs(fs) => fs.read_dir(path),
            Self::Iso9660(fs) => fs.read_dir(path),
        }
        .map_err(fs_error_to_uefi_status)?)
    }

    pub(super) fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> uefi::Result<usize> {
        Ok(match self {
            Self::Fat32(fs) => fs.read_file(path, offset, buf),
            Self::ExFat(fs) => fs.read_file(path, offset, buf),
            Self::Ext4(fs) => fs.read_file(path, offset, buf),
            Self::Ntfs(fs) => fs.read_file(path, offset, buf),
            Self::Udf(fs) => fs.read_file(path, offset, buf),
            Self::Xfs(fs) => fs.read_file(path, offset, buf),
            Self::Iso9660(fs) => fs.read_file(path, offset, buf),
        }
        .map_err(fs_error_to_uefi_status)?)
    }

    pub(super) fn file_extents(&self, path: &str) -> uefi::Result<Vec<FileExtent>> {
        Ok(match self {
            Self::Fat32(fs) => fs.file_extents(path),
            Self::ExFat(fs) => fs.file_extents(path),
            Self::Ext4(fs) => fs.file_extents(path),
            Self::Ntfs(fs) => fs.file_extents(path),
            Self::Udf(fs) => fs.file_extents(path),
            Self::Xfs(fs) => fs.file_extents(path),
            Self::Iso9660(fs) => fs.file_extents(path),
        }
        .map_err(fs_error_to_uefi_status)?)
    }

    pub(super) fn block_size(&self) -> u32 {
        match self {
            Self::Fat32(fs) => fs.block_size(),
            Self::ExFat(fs) => fs.block_size(),
            Self::Ext4(fs) => fs.block_size(),
            Self::Ntfs(fs) => fs.block_size(),
            Self::Udf(fs) => fs.block_size(),
            Self::Xfs(fs) => fs.block_size(),
            Self::Iso9660(fs) => fs.block_size(),
        }
    }
}
