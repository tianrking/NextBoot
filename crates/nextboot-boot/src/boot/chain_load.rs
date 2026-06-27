use super::candidates::{generic_efi_boot_paths, WINDOWS_BOOTMGFW_PATH};
use super::load_file::RawLoadedImage;
use super::{BootManager, VirtualBootDevice};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use log::{info, warn};
use nextboot_virtio::protocol::append_file_path_device_path;
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::table::boot::LoadImageSource;
use uefi::{Handle, Status};

impl BootManager<'_> {
    /// 引导 Windows ISO
    pub(super) fn boot_windows(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Booting Windows ISO...");
        match self.chain_load_path(device, WINDOWS_BOOTMGFW_PATH) {
            Ok(()) => return Ok(()),
            Err(err) => warn!(
                "Windows Boot Manager chain-load failed with {:?}; trying default EFI paths",
                err.status()
            ),
        }

        match self.try_chain_load_paths(device, generic_efi_boot_paths()) {
            Ok(()) => Ok(()),
            Err(chain_err) => {
                warn!(
                    "Windows default EFI chain-load paths failed with {:?}; trying WIMBOOT fallback",
                    chain_err.status()
                );
                match self.prepare_windows_iso_wimboot() {
                    Ok(()) => Ok(()),
                    Err(wimboot_err) => {
                        warn!(
                            "Windows ISO WIMBOOT fallback failed with {:?}",
                            wimboot_err.status()
                        );
                        Err(chain_err)
                    }
                }
            }
        }
    }

    /// 通用引导 (尝试链式加载)
    pub(super) fn boot_generic(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Attempting generic boot...");
        self.try_chain_load_paths(device, generic_efi_boot_paths())
    }

    pub(super) fn try_chain_load_paths(
        &self,
        device: &VirtualBootDevice,
        paths: &[&str],
    ) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;

        for path in paths {
            match self.chain_load_path(device, path) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_status = err.status();
                    warn!("Chain-load path {} failed with {:?}", path, err.status());
                }
            }
        }

        Err(last_status.into())
    }

    pub(super) fn try_load_image_paths(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        paths: &[&str],
        label: &str,
    ) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;

        for path in paths {
            match self.load_image_from_device_path(device_handle, device_path, path, label) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "LoadImage path {} on {} failed with {:?}",
                        path,
                        label,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }

    pub(super) fn chain_load_path(
        &self,
        device: &VirtualBootDevice,
        path: &str,
    ) -> uefi::Result<()> {
        let data = self.load_file(path)?;
        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        self.chain_load_with_options(device, path, &data, None)
    }

    /// 链式加载 EFI 文件
    pub(super) fn chain_load_with_options(
        &self,
        device: &VirtualBootDevice,
        path: &str,
        data: &[u8],
        load_options: Option<&str>,
    ) -> uefi::Result<()> {
        info!("Chain loading: {} ({} bytes)", path, data.len());

        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        let full_path = append_file_path_device_path(&device.device_path, path)
            .ok_or(Status::INVALID_PARAMETER)?;
        let file_path =
            unsafe { DevicePath::from_ffi_ptr(full_path.as_ptr().cast::<FfiDevicePath>()) };

        let image = self.bt.load_image(
            self.parent_image,
            LoadImageSource::FromBuffer {
                buffer: data,
                file_path: Some(file_path),
            },
        )?;

        let load_options = match load_options {
            Some(options) => Some(LoadOptionsBuffer::new(options)?),
            None => None,
        };
        if let Err(err) = self.patch_loaded_image(
            image,
            device.handle,
            full_path.as_ptr().cast::<FfiDevicePath>(),
            load_options.as_ref(),
        ) {
            warn!(
                "Failed to rebind LoadedImage source/options for {}: {:?}",
                path,
                err.status()
            );
        }

        info!("Loaded chained EFI image {:?} from {}", image, path);
        match self.bt.start_image(image) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "StartImage failed for chained image {}: {:?}",
                    path,
                    err.status()
                );
                let _ = self.bt.unload_image(image);
                Err(err)
            }
        }
    }

    pub(super) fn load_image_from_device_path(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        path: &str,
        label: &str,
    ) -> uefi::Result<()> {
        self.load_image_from_device_path_with_options(device_handle, device_path, path, label, None)
    }

    pub(super) fn load_image_from_device_path_with_options(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        path: &str,
        label: &str,
        load_options: Option<&str>,
    ) -> uefi::Result<()> {
        let full_path =
            append_file_path_device_path(device_path, path).ok_or(Status::INVALID_PARAMETER)?;
        let full_device_path =
            unsafe { DevicePath::from_ffi_ptr(full_path.as_ptr().cast::<FfiDevicePath>()) };

        info!("Trying {} EFI loader path: {}", label, path);
        let image = self.bt.load_image(
            self.parent_image,
            LoadImageSource::FromDevicePath {
                device_path: full_device_path,
                from_boot_manager: true,
            },
        )?;

        let load_options = match load_options {
            Some(options) => Some(LoadOptionsBuffer::new(options)?),
            None => None,
        };
        if let Err(err) = self.patch_loaded_image(
            image,
            device_handle,
            full_path.as_ptr().cast::<FfiDevicePath>(),
            load_options.as_ref(),
        ) {
            warn!(
                "Failed to rebind LoadedImage source/options for {} on {}: {:?}",
                path,
                label,
                err.status()
            );
        }

        info!("Loaded EFI image {:?} from {} path {}", image, label, path);
        match self.bt.start_image(image) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "StartImage failed for {} on {} with {:?}",
                    path,
                    label,
                    err.status()
                );
                let _ = self.bt.unload_image(image);
                Err(err)
            }
        }
    }

    fn patch_loaded_image(
        &self,
        image: Handle,
        source_device: Handle,
        file_path: *const FfiDevicePath,
        load_options: Option<&LoadOptionsBuffer>,
    ) -> uefi::Result<()> {
        let mut loaded_image = self.bt.open_protocol_exclusive::<RawLoadedImage>(image)?;
        loaded_image.0.device_handle = source_device.as_ptr();
        loaded_image.0.file_path = file_path;
        if let Some(load_options) = load_options {
            loaded_image.0.load_options_size = load_options.size_bytes();
            loaded_image.0.load_options = load_options.as_ptr();
        } else {
            loaded_image.0.load_options_size = 0;
            loaded_image.0.load_options = ptr::null();
        }
        Ok(())
    }
}

struct LoadOptionsBuffer {
    data: Vec<u16>,
}

impl LoadOptionsBuffer {
    fn new(options: &str) -> uefi::Result<Self> {
        if options.bytes().any(|byte| byte == 0) {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let mut data = Vec::new();
        data.try_reserve_exact(options.len().saturating_add(1))
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.extend(options.encode_utf16());
        data.push(0);

        let _ = u32::try_from(
            data.len()
                .checked_mul(core::mem::size_of::<u16>())
                .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
        )
        .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        Ok(Self { data })
    }

    fn size_bytes(&self) -> u32 {
        u32::try_from(self.data.len() * core::mem::size_of::<u16>()).unwrap_or(u32::MAX)
    }

    fn as_ptr(&self) -> *const c_void {
        self.data.as_ptr().cast::<c_void>()
    }
}
