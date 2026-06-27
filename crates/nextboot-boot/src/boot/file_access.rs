use super::candidates::WIMBOOT_XZ_MAX_OUTPUT_SIZE;
use super::errors::{fs_error_to_uefi_status, xz_error_to_uefi_status};
use super::source_volume::{
    IsoMappedFileMetadata, SourceVolumeFile, SourceVolumeFileMetadata, SourceVolumeFileSystem,
};
use super::util::normalize_iso_path;
use super::BootManager;
use crate::virtual_fs::VirtualIsoFilesystem;
use crate::xz;
use alloc::string::String;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_fs::{FileInfo, FsError};
use uefi::proto::media::block::BlockIO;
use uefi::Status;

impl BootManager<'_> {
    /// 从 ISO 加载文件
    pub(super) fn load_file(&self, path: &str) -> uefi::Result<Vec<u8>> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        info!("Loading file: {}", path);

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;

        let file = self.load_iso_file_from_fs(&fs, path)?;
        info!(
            "Loaded {} bytes from ISO path {}",
            file.data.len(),
            file.path
        );

        Ok(file.data)
    }

    pub(super) fn load_iso_text_file(&self, path: &str, max_size: usize) -> uefi::Result<String> {
        let data = self.load_file(path)?;
        if data.len() > max_size {
            return Err(Status::OUT_OF_RESOURCES.into());
        }

        let text = core::str::from_utf8(&data).map_err(|_| Status::LOAD_ERROR)?;
        Ok(String::from(text))
    }

    pub(super) fn read_iso_dir(&self, path: &str) -> uefi::Result<Vec<FileInfo>> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;
        fs.read_dir(&normalize_iso_path(path))
            .map_err(fs_error_to_uefi_status)
            .map_err(Into::into)
    }

    pub(super) fn iso_file_exists(&self, path: &str) -> uefi::Result<bool> {
        if !self.iso.image_format.is_iso() {
            return Ok(false);
        }

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;
        match fs.stat(&normalize_iso_path(path)) {
            Ok(info) => Ok(!info.is_dir),
            Err(FsError::FileNotFound | FsError::DirectoryNotFound) => Ok(false),
            Err(err) => Err(fs_error_to_uefi_status(err).into()),
        }
    }

    pub(super) fn first_existing_iso_file(
        &self,
        candidates: &[&str],
    ) -> uefi::Result<Option<String>> {
        for path in candidates {
            if self.iso_file_exists(path)? {
                return Ok(Some(normalize_iso_path(path)));
            }
        }

        Ok(None)
    }

    pub(super) fn find_iso_file_metadata(
        &self,
        fs: &VirtualIsoFilesystem,
        candidates: &[&str],
    ) -> uefi::Result<IsoMappedFileMetadata> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.iso_file_metadata(fs, path) {
                Ok(file) => {
                    info!(
                        "Found ISO file {} ({} bytes, {} mapped segment(s))",
                        file.path,
                        file.size,
                        file.segments.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!("ISO file candidate {} failed: {:?}", path, err.status());
                }
            }
        }

        Err(last_status.into())
    }

    pub(super) fn find_optional_iso_file_data(
        &self,
        fs: &VirtualIsoFilesystem,
        candidates: &[&str],
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_iso_file_from_fs(fs, path) {
                Ok(file) => {
                    info!(
                        "Loaded optional ISO file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(Some(file));
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Optional ISO file candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        if last_status == Status::NOT_FOUND {
            Ok(None)
        } else {
            Err(last_status.into())
        }
    }

    pub(super) fn load_iso_file_from_fs(
        &self,
        fs: &VirtualIsoFilesystem,
        path: &str,
    ) -> uefi::Result<SourceVolumeFile> {
        let path = normalize_iso_path(path);
        let info = fs.stat(&path).map_err(fs_error_to_uefi_status)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let file_size = usize::try_from(info.size).map_err(|_| Status::OUT_OF_RESOURCES)?;
        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        data.resize(file_size, 0);

        let read = fs
            .read_file(&path, 0, &mut data)
            .map_err(fs_error_to_uefi_status)?;
        data.truncate(read);
        Ok(SourceVolumeFile { path, data })
    }

    pub(super) fn iso_file_metadata(
        &self,
        fs: &VirtualIsoFilesystem,
        path: &str,
    ) -> uefi::Result<IsoMappedFileMetadata> {
        let path = normalize_iso_path(path);
        let info = fs.stat(&path).map_err(fs_error_to_uefi_status)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let file_extents = fs.file_extents(&path).map_err(fs_error_to_uefi_status)?;
        let segments = self.map_iso_file_extents_to_source_segments(
            fs.block_size(),
            info.size,
            &file_extents,
        )?;

        Ok(IsoMappedFileMetadata {
            path,
            size: info.size,
            segments,
        })
    }

    pub(super) fn load_source_volume_file(&self, path: &str) -> uefi::Result<SourceVolumeFile> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.asset_volume_handle)?;
        let fs = SourceVolumeFileSystem::open(&source_block_io, self.iso.asset_source_disk)?;
        fs.load_file(path)
    }

    pub(super) fn load_compressed_source_volume_file(
        &self,
        path: &str,
    ) -> uefi::Result<SourceVolumeFile> {
        let compressed = self.load_source_volume_file(path)?;
        let data = xz::decompress_xz(&compressed.data, WIMBOOT_XZ_MAX_OUTPUT_SIZE)
            .map_err(xz_error_to_uefi_status)?;
        let mut decompressed_path = compressed.path.clone();
        if decompressed_path.ends_with(".xz") {
            decompressed_path.truncate(decompressed_path.len() - 3);
        }

        Ok(SourceVolumeFile {
            path: decompressed_path,
            data,
        })
    }

    pub(super) fn find_source_volume_file(
        &self,
        candidates: &[&str],
        compressed_candidates: &[&str],
    ) -> uefi::Result<SourceVolumeFile> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded source volume file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!("Source volume file {} failed: {:?}", path, err.status());
                }
            }
        }

        for path in compressed_candidates {
            match self.load_compressed_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded compressed source volume file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Compressed source volume file {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }

    pub(super) fn source_volume_file_metadata(
        &self,
        path: &str,
    ) -> uefi::Result<SourceVolumeFileMetadata> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.asset_volume_handle)?;
        let fs = SourceVolumeFileSystem::open(&source_block_io, self.iso.asset_source_disk)?;
        fs.file_metadata(path)
    }

    pub(super) fn find_optional_source_volume_file_metadata(
        &self,
        candidates: &[&str],
    ) -> uefi::Result<Option<SourceVolumeFileMetadata>> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.source_volume_file_metadata(path) {
                Ok(file) => {
                    info!(
                        "Found optional source volume file {} ({} bytes)",
                        file.path, file.size
                    );
                    return Ok(Some(file));
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Optional source volume candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        if last_status == Status::NOT_FOUND {
            Ok(None)
        } else {
            Err(last_status.into())
        }
    }
}
