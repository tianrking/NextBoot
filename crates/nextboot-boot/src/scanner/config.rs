use super::paths::to_uefi_relative_path;
use super::IsoScanner;
use alloc::vec::Vec;
use nextboot_fs::{FileSystem, FsError};
use uefi::data_types::CString16;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;

use crate::ventoy_config::{VentoyConfig, VentoyConfigError};

const VENTOY_CONFIG_PATH: &str = "/ventoy/ventoy.json";
const VENTOY_CONFIG_MAX_SIZE: usize = 256 * 1024;

impl<'a> IsoScanner<'a> {
    pub(super) fn load_ventoy_config(&self, fs: &mut SimpleFileSystem) -> VentoyConfig {
        match self.read_ventoy_config(fs) {
            Ok(config) => config,
            Err(VentoyConfigError::NotFound) => VentoyConfig::default(),
            Err(err) => {
                log::warn!("Ignoring {}: {:?}", VENTOY_CONFIG_PATH, err);
                VentoyConfig::default()
            }
        }
    }

    fn read_ventoy_config(
        &self,
        fs: &mut SimpleFileSystem,
    ) -> Result<VentoyConfig, VentoyConfigError> {
        let mut root = fs
            .open_volume()
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        let uefi_path = to_uefi_relative_path(VENTOY_CONFIG_PATH);
        let c_path =
            CString16::try_from(uefi_path.as_str()).map_err(|_| VentoyConfigError::InvalidJson)?;
        let handle = root
            .open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())
            .map_err(|_| VentoyConfigError::NotFound)?;
        let mut file = handle
            .into_regular_file()
            .ok_or(VentoyConfigError::InvalidJson)?;
        let info = file
            .get_boxed_info::<FileInfo>()
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        let file_size =
            usize::try_from(info.file_size()).map_err(|_| VentoyConfigError::FileTooLarge)?;
        if file_size > VENTOY_CONFIG_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        data.resize(file_size, 0);
        let mut offset = 0;
        while offset < data.len() {
            let read = file
                .read(&mut data[offset..])
                .map_err(|_| VentoyConfigError::InvalidJson)?;
            if read == 0 {
                break;
            }
            offset += read;
        }
        data.truncate(offset);

        VentoyConfig::parse(&data)
    }

    pub(super) fn load_block_ventoy_config<F: FileSystem>(&self, fs: &F) -> VentoyConfig {
        match self.read_block_ventoy_config(fs) {
            Ok(config) => config,
            Err(VentoyConfigError::NotFound) => VentoyConfig::default(),
            Err(err) => {
                log::warn!("Ignoring {} {}: {:?}", F::FS_TYPE, VENTOY_CONFIG_PATH, err);
                VentoyConfig::default()
            }
        }
    }

    fn read_block_ventoy_config<F: FileSystem>(
        &self,
        fs: &F,
    ) -> Result<VentoyConfig, VentoyConfigError> {
        let info = fs.stat(VENTOY_CONFIG_PATH).map_err(|err| match err {
            FsError::FileNotFound | FsError::DirectoryNotFound => VentoyConfigError::NotFound,
            _ => VentoyConfigError::InvalidJson,
        })?;
        if info.is_dir {
            return Err(VentoyConfigError::InvalidJson);
        }

        let file_size = usize::try_from(info.size).map_err(|_| VentoyConfigError::FileTooLarge)?;
        if file_size > VENTOY_CONFIG_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        data.resize(file_size, 0);
        let read = fs
            .read_file(VENTOY_CONFIG_PATH, 0, &mut data)
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        data.truncate(read);

        VentoyConfig::parse(&data)
    }
}
