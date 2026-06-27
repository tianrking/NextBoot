//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::init::StorageDevice;
use crate::scanner::{ImageFormat, IsoExtent, IsoFile, OsType};
use crate::vdi;
use crate::vhdx;
use crate::virtual_fs::{IsoSimpleFileSystemProtocol, RegisteredIsoSimpleFileSystem};
use crate::wimboot::{self, WimbootCallbacks, WimbootVirtualFile};
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::{BlockIoOps, FileSystem, FsError};
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::protocol::append_file_path_device_path;
use nextboot_virtio::{
    PhysicalReader, VirtIoError, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType,
};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::unsafe_protocol;
use uefi::table::boot::{
    BootServices, LoadImageSource, MemoryType, OpenProtocolAttributes, OpenProtocolParams,
    ScopedProtocol, SearchType,
};
use uefi::table::runtime::{RuntimeServices, VariableAttributes, VariableVendor};
use uefi::{CString16, Guid, Handle, Identify, Status};

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("56ec3091-954c-11d2-8e3f-00a0c969723b")]
struct LoadFile(LoadFileProtocol);

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("4006c0c1-fcb3-403e-996d-4a6c8724e06d")]
struct LoadFile2(LoadFile2Protocol);

#[derive(Debug)]
#[repr(C)]
struct LoadFileProtocol {
    load_file: extern "efiapi" fn(
        *mut LoadFileProtocol,
        *const FfiDevicePath,
        bool,
        *mut usize,
        *mut c_void,
    ) -> Status,
}

#[derive(Debug)]
#[repr(C)]
struct LoadFile2Protocol {
    load_file: extern "efiapi" fn(
        *mut LoadFile2Protocol,
        *const FfiDevicePath,
        bool,
        *mut usize,
        *mut c_void,
    ) -> Status,
}

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("5b1b31a1-9562-11d2-8e3f-00a0c969723b")]
struct RawLoadedImage(RawLoadedImageProtocol);

#[derive(Debug)]
#[repr(C)]
struct RawLoadedImageProtocol {
    revision: u32,
    parent_handle: *mut c_void,
    system_table: *const c_void,
    device_handle: *mut c_void,
    file_path: *const FfiDevicePath,
    reserved: *const c_void,
    load_options_size: u32,
    load_options: *const c_void,
    image_base: *const c_void,
    image_size: u64,
    image_code_type: MemoryType,
    image_data_type: MemoryType,
    unload: Option<unsafe extern "efiapi" fn(*mut c_void) -> Status>,
}

const EFI_BOOT_X64: &str = "\\EFI\\BOOT\\BOOTX64.EFI";
const EFI_BOOT_AA64: &str = "\\EFI\\BOOT\\BOOTAA64.EFI";
const EFI_BOOT_IA32: &str = "\\EFI\\BOOT\\BOOTIA32.EFI";
const EFI_BOOT_ARM: &str = "\\EFI\\BOOT\\BOOTARM.EFI";
const WINDOWS_BOOTMGFW_PATH: &str = "/efi/microsoft/boot/bootmgfw.efi";
const NEXTBOOT_OS_PARAM_NAME: &str = "NextBootOsParam";
const NEXTBOOT_OS_PARAM_VENDOR_GUID: Guid = uefi::guid!("c1775af2-4211-4f55-9f6f-2cc5ef5667f0");
const NEXTBOOT_OS_PARAM_MAGIC: &[u8; 8] = b"NBOSPARM";
const NEXTBOOT_OS_PARAM_VERSION: u16 = 1;
const NEXTBOOT_OS_PARAM_HEADER_SIZE: usize = 80;
const NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE: usize = 24;
const NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT: u16 = 0x0001;
const NEXTBOOT_OS_PARAM_FLAG_EL_TORITO: u16 = 0x0002;
const VHD_SECTOR_SIZE: u64 = 512;
const VHD_FOOTER_SIZE: usize = 512;
const VHD_DYNAMIC_HEADER_SIZE: usize = 1024;
const VHD_UNUSED_BAT_ENTRY: u32 = 0xFFFF_FFFF;
const WIMBOOT_MAX_CALLBACK_PATH: usize = 512;
const WIMBOOT_BOOT_WIM_CALLBACK_PATH: &str = "nb-boot-wim";

static WIMBOOT_RUNTIME_CONTEXT: AtomicPtr<WimbootRuntimeContext> = AtomicPtr::new(ptr::null_mut());

fn default_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }

    #[cfg(target_arch = "arm")]
    {
        &[EFI_BOOT_ARM, EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32]
    }

    #[cfg(target_arch = "x86")]
    {
        &[EFI_BOOT_IA32, EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_ARM]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }
}

fn generic_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(target_arch = "arm")]
    {
        &[
            "/efi/boot/bootarm.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
        ]
    }

    #[cfg(target_arch = "x86")]
    {
        &[
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }
}

/// 引导管理器
pub struct BootManager<'a> {
    bt: &'a BootServices,
    rt: &'a RuntimeServices,
    parent_image: Handle,
    device: &'a StorageDevice,
    iso: &'a IsoFile,
}

impl<'a> BootManager<'a> {
    /// 创建新的引导管理器
    pub fn new(
        bt: &'a BootServices,
        rt: &'a RuntimeServices,
        parent_image: Handle,
        device: &'a StorageDevice,
        iso: &'a IsoFile,
    ) -> Self {
        Self {
            bt,
            rt,
            parent_image,
            device,
            iso,
        }
    }

    /// 准备并执行引导
    pub fn prepare_and_boot(&self) -> uefi::Result<()> {
        info!("Preparing to boot: {}", self.iso.path);
        if self.iso.image_format.is_efi_executable() {
            return self.boot_efi_executable();
        }
        if self.iso.image_format.is_wim_container() {
            return self.prepare_wimboot();
        }

        if !self.iso.image_format.supports_virtual_disk_boot() {
            warn!(
                "Image format {} is recognized but not bootable yet: {}",
                self.iso.image_format, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let boot_config = self.boot_virtual_config();
        if let Err(err) = self.publish_os_param(&boot_config) {
            warn!(
                "Failed to publish {} for {}: {:?}",
                NEXTBOOT_OS_PARAM_NAME,
                self.iso.path,
                err.status()
            );
        }

        let virtual_device = self.create_virtual_block_io(boot_config)?;

        match self.boot_virtual_device(&virtual_device) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "Virtual device boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                );

                if !self.iso.image_format.is_iso() {
                    return Err(err);
                }

                match self.iso.os_type {
                    OsType::Windows | OsType::WinPE => self.boot_windows(&virtual_device),
                    OsType::Ubuntu
                    | OsType::Debian
                    | OsType::Fedora
                    | OsType::Arch
                    | OsType::Linux => self.boot_linux(&virtual_device),
                    OsType::Unknown => self.boot_generic(&virtual_device),
                }
            }
        }
    }

    fn boot_efi_executable(&self) -> uefi::Result<()> {
        info!("Booting selected EFI executable: {}", self.iso.path);
        let device_path = self.handle_device_path_bytes(self.iso.volume_handle)?;
        self.load_image_from_device_path(
            self.iso.volume_handle,
            &device_path,
            &self.iso.path,
            "selected EFI file",
        )
    }

    fn prepare_wimboot(&self) -> uefi::Result<()> {
        let Some(wim_info) = self.iso.wim_info else {
            warn!("WIM/ESD file has no parsed WIM metadata: {}", self.iso.path);
            return Err(Status::LOAD_ERROR.into());
        };

        if !wim_info.wimboot_supported {
            warn!(
                "WIMBOOT does not support {}: boot_index={} in_range={} compression={:?}",
                self.iso.path,
                wim_info.boot_index,
                wim_info.boot_index_in_range,
                wim_info.compression
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let runtime = self.register_wimboot_runtime_files()?;
        let boot_wim = WimbootVirtualFile::new("boot.wim", WIMBOOT_BOOT_WIM_CALLBACK_PATH)
            .map_err(|_| Status::INVALID_PARAMETER)?;
        let callbacks = runtime.callbacks();
        let load_options = wimboot::build_wimboot_command_line(
            &[boot_wim],
            Some(callbacks),
            Some(wim_info.boot_index),
        )
        .map_err(|_| Status::INVALID_PARAMETER)?;

        info!(
            "Prepared WIMBOOT load options for {}: {}",
            self.iso.path, load_options
        );
        info!(
            "Registered WIMBOOT runtime file callbacks: pfsize=0x{:x} pfread=0x{:x}",
            callbacks.file_size, callbacks.file_read
        );
        warn!("WIMBOOT helper file injection and chain-loading are not implemented yet");

        Err(Status::UNSUPPORTED.into())
    }

    fn register_wimboot_runtime_files(&self) -> uefi::Result<WimbootRuntimeRegistration<'a>> {
        let bt: &'a BootServices = self.bt;
        let source_block_io = bt.open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = UefiPhysicalReader::new(&source_block_io).ok_or(uefi::Status::DEVICE_ERROR)?;
        let file = WimbootRuntimeFile::from_iso(self.iso, WIMBOOT_BOOT_WIM_CALLBACK_PATH)?;
        let mut files = Vec::new();
        files
            .try_reserve_exact(1)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        files.push(file);

        Ok(WimbootRuntimeRegistration::install(
            WimbootRuntimeContext { reader, files },
            source_block_io,
        ))
    }

    /// 引导 Linux ISO
    fn boot_linux(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        use nextboot_linux::{LinuxBootConfig, LinuxBootloader, LinuxDistro};

        info!("Booting Linux ISO...");
        if let Ok(()) = self.try_chain_load_paths(device, generic_efi_boot_paths()) {
            return Ok(());
        }

        // 映射发行版类型
        let distro = match self.iso.os_type {
            OsType::Ubuntu => LinuxDistro::Ubuntu,
            OsType::Debian => LinuxDistro::Debian,
            OsType::Fedora => LinuxDistro::Fedora,
            OsType::Arch => LinuxDistro::Arch,
            _ => LinuxDistro::Generic,
        };

        // 创建启动配置
        let config = LinuxBootConfig::for_distro(distro, &self.iso.path);

        info!("Kernel: {}", config.kernel_path);
        info!("Initrd: {}", config.initrd_path);
        info!("Cmdline: {}", config.cmdline);

        // 创建启动器
        let mut bootloader = LinuxBootloader::new(config);

        // 加载 Kernel
        let kernel_data = self.load_file(&bootloader.config().kernel_path)?;
        bootloader
            .load_kernel(kernel_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        // 加载 Initrd
        let initrd_data = self.load_file(&bootloader.config().initrd_path)?;
        bootloader
            .load_initrd(initrd_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        warn!(
            "Direct Linux EFI handover is not implemented yet; loaded kernel={} bytes initrd={} bytes",
            bootloader.kernel_size(),
            bootloader.initrd_size()
        );
        Err(Status::UNSUPPORTED.into())
    }

    /// 引导 Windows ISO
    fn boot_windows(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Booting Windows ISO...");
        match self.chain_load_path(device, WINDOWS_BOOTMGFW_PATH) {
            Ok(()) => return Ok(()),
            Err(err) => warn!(
                "Windows Boot Manager chain-load failed with {:?}; trying default EFI paths",
                err.status()
            ),
        }

        self.try_chain_load_paths(device, generic_efi_boot_paths())
    }

    /// 通用引导 (尝试链式加载)
    fn boot_generic(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Attempting generic boot...");
        self.try_chain_load_paths(device, generic_efi_boot_paths())
    }

    fn try_chain_load_paths(&self, device: &VirtualBootDevice, paths: &[&str]) -> uefi::Result<()> {
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

    fn try_load_image_paths(
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

    fn chain_load_path(&self, device: &VirtualBootDevice, path: &str) -> uefi::Result<()> {
        let data = self.load_file(path)?;
        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        self.chain_load_with_options(device, path, &data, None)
    }

    /// 链式加载 EFI 文件
    fn chain_load_with_options(
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

    fn load_image_from_device_path(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        path: &str,
        label: &str,
    ) -> uefi::Result<()> {
        self.load_image_from_device_path_with_options(device_handle, device_path, path, label, None)
    }

    fn load_image_from_device_path_with_options(
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

    /// 从 ISO 加载文件
    fn load_file(&self, path: &str) -> uefi::Result<Vec<u8>> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        info!("Loading file: {}", path);

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let iso = self.open_virtual_iso9660(&source_block_io)?;

        let path = normalize_iso_path(path);
        let info = iso.stat(&path).map_err(fs_error_to_uefi_status)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let file_size = usize::try_from(info.size).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.resize(file_size, 0);

        let read = iso
            .read_file(&path, 0, &mut data)
            .map_err(fs_error_to_uefi_status)?;
        data.truncate(read);
        info!("Loaded {} bytes from ISO path {}", read, path);

        Ok(data)
    }

    /// 创建虚拟 Block IO
    fn create_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
    ) -> uefi::Result<VirtualBootDevice> {
        use nextboot_virtio::protocol::VirtualBlockIoProtocol;

        info!("Creating virtual Block IO...");
        let load_file_entries = self.preload_load_file_entries();

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let vbio = self.build_virtual_block_io(config, &source_block_io)?;
        let virtual_info = vbio.device_info();
        let registered = VirtualBlockIoProtocol::new(vbio).install(self.bt)?;
        let virtual_handle = registered.handle();
        let device_path = registered.device_path().to_vec();

        let simple_file_system = if self.iso.image_format.is_iso() {
            match self.install_iso_simple_file_system(&source_block_io, virtual_handle) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install SimpleFileSystem on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        } else {
            None
        };

        let load_file_protocol = if load_file_entries.is_empty() {
            None
        } else {
            match PreloadedLoadFileProtocol::install(self.bt, virtual_handle, load_file_entries) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install LoadFile protocols on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        };

        registered.leak();
        if let Some(protocol) = simple_file_system {
            protocol.leak();
        }
        if let Some(protocol) = load_file_protocol {
            protocol.leak();
        }

        info!(
            "Virtual Block IO installed on {:?}: {:?}, source extents: {}",
            virtual_handle,
            virtual_info,
            self.iso.extents.len()
        );

        Ok(VirtualBootDevice {
            handle: virtual_handle,
            device_path,
        })
    }

    fn publish_os_param(&self, config: &VirtualDeviceConfig) -> uefi::Result<()> {
        let data = self.build_os_param_payload(config)?;
        let name = CString16::try_from(NEXTBOOT_OS_PARAM_NAME)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        let vendor = VariableVendor(NEXTBOOT_OS_PARAM_VENDOR_GUID);
        let attributes =
            VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.rt
            .set_variable(name.as_ref(), &vendor, attributes, &data)?;
        info!(
            "Published {} ({} bytes, {} extent record(s))",
            NEXTBOOT_OS_PARAM_NAME,
            data.len(),
            runtime_extent_count(self.iso)
        );

        Ok(())
    }

    fn build_os_param_payload(&self, config: &VirtualDeviceConfig) -> uefi::Result<Vec<u8>> {
        let path = self.iso.path.as_bytes();
        let extent_count = runtime_extent_count(self.iso);
        let path_offset = NEXTBOOT_OS_PARAM_HEADER_SIZE;
        let path_end = path_offset
            .checked_add(path.len())
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_offset = align_up(path_end, 8).ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_len = extent_count
            .checked_mul(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let total_size = extents_offset
            .checked_add(extents_len)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;

        let mut flags = 0u16;
        if self.iso.extents.is_empty() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT;
        }
        if self.iso.boot_info.is_some() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_EL_TORITO;
        }

        let mut data = Vec::new();
        data.try_reserve_exact(total_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.extend_from_slice(NEXTBOOT_OS_PARAM_MAGIC);
        push_u16(&mut data, NEXTBOOT_OS_PARAM_VERSION);
        push_u16(&mut data, usize_to_u16(NEXTBOOT_OS_PARAM_HEADER_SIZE)?);
        push_u16(
            &mut data,
            usize_to_u16(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)?,
        );
        push_u16(&mut data, flags);
        push_u32(&mut data, usize_to_u32(total_size)?);
        push_u32(&mut data, usize_to_u32(self.iso.volume_index)?);
        push_u32(&mut data, os_type_code(self.iso.os_type));
        push_u32(&mut data, virtual_device_type_code(config.device_type));
        push_u64(&mut data, self.iso.virtual_size);
        push_u64(&mut data, self.iso.start_lba);
        push_u32(&mut data, self.iso.block_size);
        push_u32(&mut data, config.block_size);
        push_u32(&mut data, config.physical_block_size);
        push_u32(&mut data, usize_to_u32(extent_count)?);
        push_u32(&mut data, usize_to_u32(path_offset)?);
        push_u32(&mut data, usize_to_u32(path.len())?);
        push_u32(&mut data, usize_to_u32(extents_offset)?);
        push_u32(&mut data, usize_to_u32(extents_len)?);
        debug_assert_eq!(data.len(), NEXTBOOT_OS_PARAM_HEADER_SIZE);

        data.extend_from_slice(path);
        data.resize(extents_offset, 0);
        self.append_runtime_extents(&mut data)?;
        debug_assert_eq!(data.len(), total_size);

        Ok(data)
    }

    fn append_runtime_extents(&self, data: &mut Vec<u8>) -> uefi::Result<()> {
        if self.iso.extents.is_empty() {
            let block_count = div_round_up(self.iso.virtual_size, u64::from(self.iso.block_size))
                .ok_or(uefi::Status::INVALID_PARAMETER)?;
            push_extent_record(data, 0, self.iso.start_lba, block_count);
            return Ok(());
        }

        for extent in &self.iso.extents {
            push_extent_record(
                data,
                extent.virtual_block_start,
                extent.physical_lba,
                extent.block_count,
            );
        }

        Ok(())
    }

    fn boot_virtual_config(&self) -> VirtualDeviceConfig {
        use nextboot_virtio::CdRomBootInfo;

        let device_type = if self.iso.image_format.is_iso() {
            match self.iso.os_type {
                OsType::Windows | OsType::WinPE => VirtualDeviceType::DvdRom,
                _ => VirtualDeviceType::HardDisk,
            }
        } else {
            VirtualDeviceType::HardDisk
        };
        let virtual_block_size = if let Some(block_size) = self.iso.virtual_block_size {
            block_size
        } else if self.iso.image_format.uses_512_byte_virtual_sectors() {
            512
        } else {
            match device_type {
                VirtualDeviceType::DvdRom => 2048,
                _ => self.iso.block_size,
            }
        };

        let mut config = VirtualDeviceConfig::new(
            device_type,
            self.iso.start_lba,
            self.iso.virtual_size,
            virtual_block_size,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        if let Some(boot) = self.iso.boot_info {
            config = config.with_cdrom_boot(CdRomBootInfo::new(
                boot.boot_entry,
                u64::from(boot.image_lba),
                boot.image_block_count,
            ));
            info!(
                "Using EFI El Torito boot image: catalog LBA {}, entry {}, image LBA {}, blocks {}",
                boot.catalog_lba, boot.boot_entry, boot.image_lba, boot.image_block_count
            );
        } else if self.iso.image_format.is_iso() && matches!(device_type, VirtualDeviceType::DvdRom)
        {
            warn!("No EFI El Torito boot image found for {}", self.iso.path);
        }

        config
    }

    fn iso9660_virtual_config(&self) -> VirtualDeviceConfig {
        VirtualDeviceConfig::new(
            VirtualDeviceType::DvdRom,
            self.iso.start_lba,
            self.iso.size,
            2048,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path)
    }

    fn build_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let mut vbio = if self.iso.image_format == ImageFormat::DynamicVhd {
            self.build_dynamic_vhd_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vhdx {
            self.build_vhdx_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vdi {
            self.build_vdi_block_io(config, source_block_io)?
        } else if self.iso.extents.is_empty() {
            warn!(
                "No extent map for {}, falling back to contiguous LBA {}",
                self.iso.path, self.iso.start_lba
            );
            VirtualBlockIo::new(config)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = UefiPhysicalReader::new(source_block_io).ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);

        Ok(vbio)
    }

    fn build_dynamic_vhd_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let mut footer = [0u8; VHD_FOOTER_SIZE];
        let footer_offset = self
            .iso
            .size
            .checked_sub(VHD_FOOTER_SIZE as u64)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        read_vhd_file_bytes(&file_vbio, footer_offset, &mut footer)?;

        let footer = parse_dynamic_vhd_footer(&footer).ok_or(uefi::Status::LOAD_ERROR)?;
        let virtual_size = config.iso_size;
        if footer.virtual_size != virtual_size {
            warn!(
                "Dynamic VHD virtual size mismatch for {}: scanner={} footer={}",
                self.iso.path, virtual_size, footer.virtual_size
            );
        }

        let mut header = alloc::vec![0u8; VHD_DYNAMIC_HEADER_SIZE];
        read_vhd_file_bytes(&file_vbio, footer.data_offset, &mut header)?;
        let header = parse_dynamic_vhd_header(&header).ok_or(uefi::Status::LOAD_ERROR)?;
        if header.header_version != 0x0001_0000 {
            warn!(
                "Dynamic VHD header version for {} is 0x{:08x}",
                self.iso.path, header.header_version
            );
        }

        let block_size = u64::from(header.block_size);
        if virtual_size == 0 || block_size == 0 || block_size % VHD_SECTOR_SIZE != 0 {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let sectors_per_block = block_size / VHD_SECTOR_SIZE;
        let bitmap_bytes = div_round_up(sectors_per_block, 8)
            .and_then(|bytes| align_up_u64(bytes, VHD_SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let entries_needed =
            div_round_up(virtual_size, block_size).ok_or(uefi::Status::LOAD_ERROR)?;
        if entries_needed == 0 || u64::from(header.max_table_entries) < entries_needed {
            return Err(uefi::Status::LOAD_ERROR.into());
        }
        let entries_to_scan = entries_needed;

        let bat_bytes = entries_to_scan
            .checked_mul(4)
            .and_then(|bytes| align_up_u64(bytes, VHD_SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        read_vhd_file_bytes(&file_vbio, header.table_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;

        for index in 0..entries_to_scan {
            let bat_offset = usize::try_from(index.checked_mul(4).ok_or(uefi::Status::LOAD_ERROR)?)
                .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let bat_entry = read_be_u32(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            if bat_entry == VHD_UNUSED_BAT_ENTRY {
                continue;
            }

            let virtual_start = index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= virtual_size {
                break;
            }
            let byte_count = block_size.min(virtual_size - virtual_start);
            let file_offset = u64::from(bat_entry)
                .checked_mul(VHD_SECTOR_SIZE)
                .and_then(|offset| offset.checked_add(bitmap_bytes))
                .ok_or(uefi::Status::LOAD_ERROR)?;

            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(virtual_size);
        byte_mapping.optimize();
        info!(
            "Mapped dynamic VHD {}: virtual={} bytes, block={} bytes, allocated BAT entries={}, physical segments={}",
            self.iso.path,
            virtual_size,
            block_size,
            allocated_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vhdx_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let (regions, metadata) = self.read_vhdx_layout(&file_vbio)?;
        if metadata.has_parent {
            warn!(
                "VHDX parent chains are not supported yet: {}",
                self.iso.path
            );
            return Err(uefi::Status::UNSUPPORTED.into());
        }

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.logical_sector_size;

        let block_size = u64::from(metadata.block_size);
        let payload_blocks =
            vhdx::payload_block_count(metadata.virtual_disk_size, metadata.block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
        let chunk_ratio = metadata.chunk_ratio().ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_entries =
            vhdx::bat_entry_count(payload_blocks, chunk_ratio).ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_bytes = bat_entries
            .checked_mul(8)
            .and_then(|bytes| align_up_u64(bytes, VHD_SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;

        if bat_bytes > regions.bat_length {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        read_vhd_file_bytes(&file_vbio, regions.bat_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for payload_index in 0..payload_blocks {
            let bat_index = vhdx::payload_bat_index(payload_index, chunk_ratio)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let bat_offset =
                usize::try_from(bat_index.checked_mul(8).ok_or(uefi::Status::LOAD_ERROR)?)
                    .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let raw_entry = vhdx::read_le_u64(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            let entry = vhdx::parse_bat_entry(raw_entry);
            let virtual_start = payload_index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);

            match entry.state {
                vhdx::VHDX_BAT_STATE_FULLY_PRESENT => {
                    if entry
                        .file_offset
                        .checked_add(byte_count)
                        .map_or(true, |end| end > self.iso.size)
                    {
                        return Err(uefi::Status::DEVICE_ERROR.into());
                    }

                    self.map_image_file_range_to_physical(
                        &mut byte_mapping,
                        virtual_start,
                        entry.file_offset,
                        byte_count,
                    )?;
                    allocated_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_NOT_PRESENT
                | vhdx::VHDX_BAT_STATE_ZERO
                | vhdx::VHDX_BAT_STATE_UNMAPPED => {
                    zero_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_UNDEFINED | vhdx::VHDX_BAT_STATE_PARTIALLY_PRESENT => {
                    warn!(
                        "Unsupported VHDX BAT state {} at payload block {} in {}",
                        entry.state, payload_index, self.iso.path
                    );
                    return Err(uefi::Status::UNSUPPORTED.into());
                }
                _ => {
                    return Err(uefi::Status::LOAD_ERROR.into());
                }
            }
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VHDX {}: virtual={} bytes, block={} bytes, logical_sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.logical_sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vdi_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let metadata = self.read_vdi_metadata(&file_vbio)?;

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.sector_size;

        let map_bytes =
            vdi::block_map_bytes(metadata.block_count).ok_or(uefi::Status::LOAD_ERROR)?;
        if metadata
            .offset_blocks
            .checked_add(map_bytes)
            .map_or(true, |end| {
                end > self.iso.size || end > metadata.offset_data
            })
        {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let map_len = usize::try_from(map_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut block_map = Vec::new();
        block_map
            .try_reserve_exact(map_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        block_map.resize(map_len, 0);
        read_vhd_file_bytes(&file_vbio, metadata.offset_blocks, &mut block_map)?;

        let block_size = u64::from(metadata.block_size);
        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for block_index in 0..metadata.block_count {
            let virtual_start = u64::from(block_index)
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= metadata.virtual_disk_size {
                break;
            }

            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);
            let map_entry = vdi::read_block_map_entry(&block_map, block_index)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if !vdi::is_allocated_block(map_entry) {
                zero_blocks += 1;
                continue;
            }

            if map_entry >= metadata.block_count {
                warn!(
                    "Invalid VDI block map entry {} at virtual block {} in {}",
                    map_entry, block_index, self.iso.path
                );
                return Err(uefi::Status::LOAD_ERROR.into());
            }

            let file_offset = metadata
                .offset_data
                .checked_add(
                    u64::from(map_entry)
                        .checked_mul(block_size)
                        .ok_or(uefi::Status::LOAD_ERROR)?,
                )
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VDI {}: virtual={} bytes, block={} bytes, sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn read_vhdx_layout(
        &self,
        file_vbio: &VirtualBlockIo,
    ) -> uefi::Result<(vhdx::VhdxRegions, vhdx::VhdxMetadata)> {
        let mut header = Vec::new();
        header
            .try_reserve_exact(vhdx::VHDX_HEADER_SECTION_SIZE)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        header.resize(vhdx::VHDX_HEADER_SECTION_SIZE, 0);
        read_vhd_file_bytes(file_vbio, 0, &mut header)?;
        let regions = vhdx::parse_vhdx_regions(&header).ok_or(uefi::Status::LOAD_ERROR)?;

        let metadata_len =
            usize::try_from(regions.metadata_length).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut metadata = Vec::new();
        metadata
            .try_reserve_exact(metadata_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        metadata.resize(metadata_len, 0);
        read_vhd_file_bytes(file_vbio, regions.metadata_offset, &mut metadata)?;
        let metadata = vhdx::parse_vhdx_metadata(&metadata).ok_or(uefi::Status::LOAD_ERROR)?;

        Ok((regions, metadata))
    }

    fn read_vdi_metadata(&self, file_vbio: &VirtualBlockIo) -> uefi::Result<vdi::VdiMetadata> {
        let mut header = [0u8; vdi::VDI_HEADER_SIZE];
        read_vhd_file_bytes(file_vbio, 0, &mut header)?;
        vdi::parse_vdi_metadata(&header).ok_or(uefi::Status::LOAD_ERROR.into())
    }

    fn build_image_file_block_io(&self, source_block_io: &BlockIO) -> uefi::Result<VirtualBlockIo> {
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::HardDisk,
            self.iso.start_lba,
            self.iso.size,
            VHD_SECTOR_SIZE as u32,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        let mut vbio = if self.iso.extents.is_empty() {
            let source_block_size = u64::from(self.iso.block_size);
            let block_count =
                div_round_up(self.iso.size, source_block_size).ok_or(uefi::Status::LOAD_ERROR)?;
            let extents = [(0, self.iso.start_lba, block_count)];
            VirtualBlockIo::from_file_extents(config, &extents)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = UefiPhysicalReader::new(source_block_io).ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);
        Ok(vbio)
    }

    fn map_image_file_range_to_physical(
        &self,
        table: &mut ByteMappingTable,
        virtual_start: u64,
        file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let source_block_size = u64::from(self.iso.block_size);
        if source_block_size == 0 {
            return Err(uefi::Status::INVALID_PARAMETER.into());
        }

        if self.iso.extents.is_empty() {
            let physical_start = self
                .iso
                .start_lba
                .checked_mul(source_block_size)
                .and_then(|start| start.checked_add(file_offset))
                .ok_or(uefi::Status::LOAD_ERROR)?;
            table.add_segment(virtual_start, byte_count, physical_start);
            return Ok(());
        }

        let file_end = file_offset
            .checked_add(byte_count)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let mut cursor = file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &self.iso.extents {
                let extent_file_start = extent
                    .virtual_block_start
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_bytes = extent
                    .block_count
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_file_end = extent_file_start
                    .checked_add(extent_bytes)
                    .ok_or(uefi::Status::LOAD_ERROR)?;

                if cursor < extent_file_start || cursor >= extent_file_end {
                    continue;
                }

                let overlap_end = file_end.min(extent_file_end);
                let overlap_len = overlap_end - cursor;
                let physical_start = extent
                    .physical_lba
                    .checked_mul(source_block_size)
                    .and_then(|start| start.checked_add(cursor - extent_file_start))
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let segment_virtual_start = virtual_start
                    .checked_add(cursor - file_offset)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                table.add_segment(segment_virtual_start, overlap_len, physical_start);
                cursor = overlap_end;
                mapped = true;
                break;
            }

            if !mapped {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }

    fn open_virtual_iso9660(&self, source_block_io: &BlockIO) -> uefi::Result<Iso9660> {
        let config = self.iso9660_virtual_config();
        let vbio = self.build_virtual_block_io(config, source_block_io)?;
        let iso = Iso9660::open(Rc::new(VirtualIsoBlockIo::new(vbio)))
            .map_err(fs_error_to_uefi_status)?;
        Ok(iso)
    }

    fn install_iso_simple_file_system(
        &self,
        source_block_io: &BlockIO,
        virtual_handle: Handle,
    ) -> uefi::Result<RegisteredIsoSimpleFileSystem> {
        let iso = self.open_virtual_iso9660(source_block_io)?;
        let block_size = iso.block_size();
        IsoSimpleFileSystemProtocol::install(
            self.bt,
            virtual_handle,
            Rc::new(iso),
            self.iso.size,
            block_size,
        )
    }

    fn preload_load_file_entries(&self) -> Vec<PreloadedFile> {
        let mut entries = Vec::new();
        if !self.iso.image_format.is_iso() {
            return entries;
        }

        for path in generic_efi_boot_paths() {
            match self.load_file(path) {
                Ok(data) if !data.is_empty() => {
                    let key = normalize_load_file_key(path);
                    if entries
                        .iter()
                        .any(|entry: &PreloadedFile| entry.path == key)
                    {
                        continue;
                    }

                    info!("Preloaded LoadFile path {} ({} bytes)", path, data.len());
                    entries.push(PreloadedFile { path: key, data });
                }
                Ok(_) => {}
                Err(err) => {
                    info!("LoadFile preload skipped {}: {:?}", path, err.status());
                }
            }
        }

        entries
    }

    fn boot_virtual_device(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Connecting virtual boot device {:?}", device.handle);
        if let Err(err) = self.bt.connect_controller(device.handle, None, None, true) {
            warn!(
                "ConnectController on virtual device returned {:?}; trying LoadImage anyway",
                err.status()
            );
        }

        if !self.iso.image_format.is_iso() {
            match self.boot_virtual_disk_partitions(device) {
                Ok(()) => return Ok(()),
                Err(err) => warn!(
                    "Virtual disk partition boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                ),
            }
        }

        self.try_load_image_paths(
            device.handle,
            &device.device_path,
            default_efi_boot_paths(),
            "virtual device",
        )
    }

    fn boot_virtual_disk_partitions(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;
        for attempt in 0..3 {
            let fs_handles = self
                .bt
                .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
            let mut matched_partitions = 0usize;

            for handle in fs_handles.iter().copied() {
                let Ok(partition_path) = self.handle_device_path_bytes(handle) else {
                    continue;
                };

                if !is_child_device_path(&device.device_path, &partition_path) {
                    continue;
                }

                matched_partitions += 1;
                info!(
                    "Found virtual disk filesystem partition {:?} for {}",
                    handle, self.iso.path
                );

                match self.try_load_image_paths(
                    handle,
                    &partition_path,
                    default_efi_boot_paths(),
                    "virtual disk partition",
                ) {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        last_status = err.status();
                        warn!(
                            "No bootable EFI path on virtual partition {:?}: {:?}",
                            handle,
                            err.status()
                        );
                    }
                }
            }

            if matched_partitions > 0 {
                return Err(last_status.into());
            }

            warn!(
                "No SimpleFileSystem partitions found under {} (attempt {}/3)",
                self.iso.path,
                attempt + 1
            );
            self.bt.stall(2_000_000);
        }

        Err(last_status.into())
    }

    fn handle_device_path_bytes(&self, handle: Handle) -> uefi::Result<Vec<u8>> {
        let device_path = unsafe {
            self.bt.open_protocol::<DevicePath>(
                OpenProtocolParams {
                    handle,
                    agent: self.parent_image,
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }?;

        device_path_to_vec(&device_path)
    }
}

struct VirtualBootDevice {
    handle: Handle,
    device_path: Vec<u8>,
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

#[derive(Debug, Clone, Copy)]
struct DynamicVhdFooter {
    data_offset: u64,
    virtual_size: u64,
}

#[derive(Debug, Clone, Copy)]
struct DynamicVhdHeader {
    table_offset: u64,
    header_version: u32,
    max_table_entries: u32,
    block_size: u32,
}

fn read_vhd_file_bytes(vbio: &VirtualBlockIo, offset: u64, buf: &mut [u8]) -> uefi::Result<()> {
    vbio.read_bytes(vbio.media_id(), offset, buf)
        .map_err(virtio_error_to_uefi_status)?;
    Ok(())
}

fn parse_dynamic_vhd_footer(data: &[u8]) -> Option<DynamicVhdFooter> {
    if data.len() < VHD_FOOTER_SIZE || data.get(0..8)? != b"conectix" {
        return None;
    }

    let data_offset = read_be_u64(data, 16)?;
    let virtual_size = read_be_u64(data, 48)?;
    let disk_type = read_be_u32(data, 60)?;
    if data_offset == u64::MAX || disk_type != 3 {
        return None;
    }

    Some(DynamicVhdFooter {
        data_offset,
        virtual_size,
    })
}

fn parse_dynamic_vhd_header(data: &[u8]) -> Option<DynamicVhdHeader> {
    if data.len() < VHD_DYNAMIC_HEADER_SIZE || data.get(0..8)? != b"cxsparse" {
        return None;
    }

    let table_offset = read_be_u64(data, 16)?;
    let header_version = read_be_u32(data, 24)?;
    let max_table_entries = read_be_u32(data, 28)?;
    let block_size = read_be_u32(data, 32)?;

    if table_offset == u64::MAX || max_table_entries == 0 || block_size == 0 {
        return None;
    }

    Some(DynamicVhdHeader {
        table_offset,
        header_version,
        max_table_entries,
        block_size,
    })
}

fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_be_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn virtio_error_to_uefi_status(err: VirtIoError) -> Status {
    match err {
        VirtIoError::OutOfBounds | VirtIoError::InvalidMapping => Status::LOAD_ERROR,
        VirtIoError::WriteProtected => Status::WRITE_PROTECTED,
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => Status::INVALID_PARAMETER,
        VirtIoError::MediaChanged => Status::MEDIA_CHANGED,
        VirtIoError::NoPhysicalRead => Status::NO_MEDIA,
        VirtIoError::CrcError => Status::CRC_ERROR,
        VirtIoError::ReadFailed | VirtIoError::DeviceError => Status::DEVICE_ERROR,
    }
}

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

struct WimbootRuntimeContext {
    reader: UefiPhysicalReader,
    files: Vec<WimbootRuntimeFile>,
}

impl WimbootRuntimeContext {
    fn find_file(&self, path: &[u8]) -> Option<&WimbootRuntimeFile> {
        self.files
            .iter()
            .find(|file| file.callback_path.as_bytes() == path)
    }
}

struct WimbootRuntimeFile {
    callback_path: String,
    size: u64,
    block_size: u32,
    extents: Vec<IsoExtent>,
}

impl WimbootRuntimeFile {
    fn from_iso(iso: &IsoFile, callback_path: &str) -> uefi::Result<Self> {
        if iso.extents.is_empty() || iso.block_size == 0 {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size: iso.size,
            block_size: iso.block_size,
            extents: iso.extents.clone(),
        })
    }

    fn size_i32(&self) -> Option<i32> {
        i32::try_from(self.size).ok()
    }

    fn read_range(&self, reader: &UefiPhysicalReader, offset: u64, buf: &mut [u8]) -> Option<()> {
        let end = offset.checked_add(buf.len() as u64)?;
        if end > self.size {
            return None;
        }

        let block_size = u64::from(self.block_size);
        let mut cursor = offset;
        let mut copied = 0usize;

        while copied < buf.len() {
            let extent = self.extents.iter().find(|extent| {
                let Some(extent_start) = extent.virtual_block_start.checked_mul(block_size) else {
                    return false;
                };
                let Some(extent_bytes) = extent.block_count.checked_mul(block_size) else {
                    return false;
                };
                let Some(extent_end) = extent_start.checked_add(extent_bytes) else {
                    return false;
                };
                cursor >= extent_start && cursor < extent_end
            })?;

            let extent_start = extent.virtual_block_start.checked_mul(block_size)?;
            let extent_bytes = extent.block_count.checked_mul(block_size)?;
            let extent_end = extent_start.checked_add(extent_bytes)?;
            let read_end = end.min(extent_end);
            let read_len = usize::try_from(read_end.checked_sub(cursor)?).ok()?;
            let physical_byte = extent
                .physical_lba
                .checked_mul(block_size)?
                .checked_add(cursor.checked_sub(extent_start)?)?;

            read_physical_bytes(
                reader,
                self.block_size,
                physical_byte,
                &mut buf[copied..copied + read_len],
            )?;

            cursor = read_end;
            copied += read_len;
        }

        Some(())
    }
}

struct WimbootRuntimeRegistration<'a> {
    context: *mut WimbootRuntimeContext,
    previous: *mut WimbootRuntimeContext,
    _source_block_io: ScopedProtocol<'a, BlockIO>,
}

impl<'a> WimbootRuntimeRegistration<'a> {
    fn install(
        context: WimbootRuntimeContext,
        source_block_io: ScopedProtocol<'a, BlockIO>,
    ) -> Self {
        let context = Box::into_raw(Box::new(context));
        let previous = WIMBOOT_RUNTIME_CONTEXT.swap(context, Ordering::SeqCst);
        Self {
            context,
            previous,
            _source_block_io: source_block_io,
        }
    }

    fn callbacks(&self) -> WimbootCallbacks {
        WimbootCallbacks {
            file_size: wimboot_runtime_file_size as usize,
            file_read: wimboot_runtime_file_read as usize,
        }
    }
}

impl Drop for WimbootRuntimeRegistration<'_> {
    fn drop(&mut self) {
        if WIMBOOT_RUNTIME_CONTEXT.load(Ordering::SeqCst) == self.context {
            WIMBOOT_RUNTIME_CONTEXT.store(self.previous, Ordering::SeqCst);
        }

        unsafe {
            drop(Box::from_raw(self.context));
        }
    }
}

extern "C" fn wimboot_runtime_file_size(path: *const u8) -> i32 {
    let Some(context) = current_wimboot_context() else {
        return -1;
    };
    let Some(path) = (unsafe { c_path_bytes(path) }) else {
        return -1;
    };
    let Some(file) = context.find_file(path) else {
        return -1;
    };

    file.size_i32().unwrap_or(-1)
}

extern "C" fn wimboot_runtime_file_read(
    path: *const u8,
    offset: i32,
    len: i32,
    buf: *mut c_void,
) -> i32 {
    if offset < 0 || len < 0 {
        return -1;
    }

    let len = len as usize;
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }

    let Some(context) = current_wimboot_context() else {
        return -1;
    };
    let Some(path) = (unsafe { c_path_bytes(path) }) else {
        return -1;
    };
    let Some(file) = context.find_file(path) else {
        return -1;
    };

    let data = unsafe { core::slice::from_raw_parts_mut(buf.cast::<u8>(), len) };
    match file.read_range(&context.reader, offset as u64, data) {
        Some(()) => len.try_into().unwrap_or(i32::MAX),
        None => -1,
    }
}

fn current_wimboot_context() -> Option<&'static WimbootRuntimeContext> {
    let context = WIMBOOT_RUNTIME_CONTEXT.load(Ordering::SeqCst);
    if context.is_null() {
        None
    } else {
        Some(unsafe { &*context })
    }
}

unsafe fn c_path_bytes(path: *const u8) -> Option<&'static [u8]> {
    if path.is_null() {
        return None;
    }

    for len in 0..WIMBOOT_MAX_CALLBACK_PATH {
        let byte = unsafe { *path.add(len) };
        if byte == 0 {
            return Some(unsafe { core::slice::from_raw_parts(path, len) });
        }
    }

    None
}

fn read_physical_bytes(
    reader: &UefiPhysicalReader,
    block_size: u32,
    physical_byte_start: u64,
    buf: &mut [u8],
) -> Option<()> {
    let block_size = usize::try_from(block_size).ok()?;
    if block_size == 0 {
        return None;
    }

    let mut scratch = Vec::new();
    scratch.try_reserve_exact(block_size).ok()?;
    scratch.resize(block_size, 0);

    let mut physical_byte = physical_byte_start;
    let mut copied = 0usize;

    while copied < buf.len() {
        let physical_lba = physical_byte / block_size as u64;
        let in_block_offset = usize::try_from(physical_byte % block_size as u64).ok()?;
        let copy_size = (block_size - in_block_offset).min(buf.len() - copied);

        reader.read_blocks(physical_lba, &mut scratch).ok()?;
        buf[copied..copied + copy_size]
            .copy_from_slice(&scratch[in_block_offset..in_block_offset + copy_size]);

        physical_byte = physical_byte.checked_add(copy_size as u64)?;
        copied += copy_size;
    }

    Some(())
}

struct PreloadedFile {
    path: String,
    data: Vec<u8>,
}

#[repr(C)]
struct PreloadedLoadFileProtocol {
    load_file: LoadFileProtocol,
    load_file_2: LoadFile2Protocol,
    entries: Vec<PreloadedFile>,
}

impl PreloadedLoadFileProtocol {
    fn install(
        bt: &BootServices,
        handle: Handle,
        entries: Vec<PreloadedFile>,
    ) -> uefi::Result<RegisteredPreloadedLoadFile> {
        let mut protocol = alloc::boxed::Box::new(Self {
            load_file: LoadFileProtocol {
                load_file: Self::load_file_handler,
            },
            load_file_2: LoadFile2Protocol {
                load_file: Self::load_file_2_handler,
            },
            entries,
        });

        let load_file_interface = protocol.load_file_ptr().cast::<c_void>();
        unsafe {
            bt.install_protocol_interface(Some(handle), &LoadFile::GUID, load_file_interface)
        }?;

        let load_file_2_interface = protocol.load_file_2_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &LoadFile2::GUID, load_file_2_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &LoadFile::GUID, load_file_interface)
            };
            return Err(err);
        }

        Ok(RegisteredPreloadedLoadFile { protocol })
    }

    fn load_file_ptr(&mut self) -> *mut LoadFileProtocol {
        &mut self.load_file
    }

    fn load_file_2_ptr(&mut self) -> *mut LoadFile2Protocol {
        &mut self.load_file_2
    }

    extern "efiapi" fn load_file_handler(
        this: *mut LoadFileProtocol,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        let Some(protocol) = Self::from_load_file(this) else {
            return Status::INVALID_PARAMETER;
        };

        protocol.load_file(file_path, boot_policy, buffer_size, buffer)
    }

    extern "efiapi" fn load_file_2_handler(
        this: *mut LoadFile2Protocol,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        let Some(protocol) = Self::from_load_file_2(this) else {
            return Status::INVALID_PARAMETER;
        };

        protocol.load_file(file_path, boot_policy, buffer_size, buffer)
    }

    fn load_file(
        &self,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        if buffer_size.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let requested = unsafe { load_file_path_from_device_path(file_path) };
        let entry = requested
            .as_ref()
            .and_then(|path| self.find_entry(path))
            .or_else(|| boot_policy.then(|| self.entries.first()).flatten());

        let Some(entry) = entry else {
            return Status::NOT_FOUND;
        };

        let required_size = entry.data.len();
        let provided_size = unsafe { *buffer_size };
        unsafe {
            *buffer_size = required_size;
        }

        if buffer.is_null() || provided_size < required_size {
            return Status::BUFFER_TOO_SMALL;
        }

        unsafe {
            ptr::copy_nonoverlapping(entry.data.as_ptr(), buffer.cast::<u8>(), required_size);
        }

        Status::SUCCESS
    }

    fn find_entry(&self, path: &str) -> Option<&PreloadedFile> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    fn from_load_file(this: *mut LoadFileProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        Some(unsafe { &mut *(this.cast::<Self>()) })
    }

    fn from_load_file_2(this: *mut LoadFile2Protocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, load_file_2);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
    }
}

struct RegisteredPreloadedLoadFile {
    protocol: alloc::boxed::Box<PreloadedLoadFileProtocol>,
}

impl RegisteredPreloadedLoadFile {
    fn leak(self) {
        let _ = alloc::boxed::Box::leak(self.protocol);
    }
}

struct VirtualIsoBlockIo {
    vbio: VirtualBlockIo,
    media_id: u32,
}

impl VirtualIsoBlockIo {
    fn new(vbio: VirtualBlockIo) -> Self {
        let media_id = vbio.media_id();
        Self { vbio, media_id }
    }
}

impl BlockIoOps for VirtualIsoBlockIo {
    fn block_size(&self) -> u32 {
        self.vbio.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.vbio.block_count()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.vbio
            .read_blocks(self.media_id, lba, buf)
            .map_err(virtio_error_to_fs_error)
    }
}

fn virtio_error_to_fs_error(err: VirtIoError) -> FsError {
    match err {
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => FsError::InvalidArgument,
        VirtIoError::OutOfBounds
        | VirtIoError::MediaChanged
        | VirtIoError::InvalidMapping
        | VirtIoError::NoPhysicalRead
        | VirtIoError::ReadFailed
        | VirtIoError::DeviceError
        | VirtIoError::CrcError => FsError::ReadError,
        VirtIoError::WriteProtected => FsError::UnsupportedFs,
    }
}

fn fs_error_to_uefi_status(err: FsError) -> Status {
    match err {
        FsError::FileNotFound | FsError::DirectoryNotFound => Status::NOT_FOUND,
        FsError::InvalidPath | FsError::InvalidArgument => Status::INVALID_PARAMETER,
        FsError::OutOfMemory | FsError::FileTooLarge => Status::OUT_OF_RESOURCES,
        FsError::NotDirectory | FsError::NotFile | FsError::UnsupportedFs => Status::UNSUPPORTED,
        FsError::InvalidSignature | FsError::BlockSizeMismatch | FsError::Corrupted => {
            Status::LOAD_ERROR
        }
        FsError::ReadError => Status::DEVICE_ERROR,
    }
}

fn normalize_iso_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = trimmed.trim_start_matches(['/', '\\']);
    let mut normalized = String::from("/");
    let mut first = true;

    for part in trimmed
        .split(|ch| ch == '/' || ch == '\\')
        .filter(|part| !part.is_empty())
    {
        if !first {
            normalized.push('/');
        }
        normalized.push_str(part);
        first = false;
    }

    normalized
}

fn normalize_load_file_key(path: &str) -> String {
    let mut normalized = normalize_iso_path(path);
    normalized.make_ascii_lowercase();
    normalized
}

fn runtime_extent_count(iso: &IsoFile) -> usize {
    if iso.extents.is_empty() {
        1
    } else {
        iso.extents.len()
    }
}

fn device_path_to_vec(device_path: &DevicePath) -> uefi::Result<Vec<u8>> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }.ok_or(uefi::Status::INVALID_PARAMETER)?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Ok(bytes.to_vec())
}

unsafe fn device_path_byte_len(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }

    let mut offset = 0usize;
    loop {
        let node = ptr.add(offset);
        let node_type = ptr::read_unaligned(node);
        let node_subtype = ptr::read_unaligned(node.add(1));
        let len_lo = ptr::read_unaligned(node.add(2));
        let len_hi = ptr::read_unaligned(node.add(3));
        let node_len = u16::from_le_bytes([len_lo, len_hi]) as usize;
        if node_len < 4 {
            return None;
        }

        offset = offset.checked_add(node_len)?;
        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }
    }
}

fn is_child_device_path(parent: &[u8], child: &[u8]) -> bool {
    let parent_prefix_len = parent_without_end_len(parent).unwrap_or(parent.len());
    child.len() >= parent_prefix_len
        && child.get(..parent_prefix_len) == parent.get(..parent_prefix_len)
}

fn parent_without_end_len(path: &[u8]) -> Option<usize> {
    if path.len() < 4 {
        return None;
    }

    let mut offset = 0usize;
    while offset.checked_add(4)? <= path.len() {
        let node_type = *path.get(offset)?;
        let node_subtype = *path.get(offset + 1)?;
        let node_len =
            u16::from_le_bytes([*path.get(offset + 2)?, *path.get(offset + 3)?]) as usize;
        if node_len < 4 || offset.checked_add(node_len)? > path.len() {
            return None;
        }

        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }

        offset += node_len;
    }

    None
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }

    value.checked_add(divisor - 1).map(|value| value / divisor)
}

fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    if align == 0 {
        return None;
    }

    let remainder = value % align;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(align - remainder)
    }
}

fn usize_to_u16(value: usize) -> uefi::Result<u16> {
    u16::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

fn usize_to_u32(value: usize) -> uefi::Result<u32> {
    u32::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

fn os_type_code(os_type: OsType) -> u32 {
    match os_type {
        OsType::Unknown => 0,
        OsType::Windows => 1,
        OsType::WinPE => 2,
        OsType::Linux => 10,
        OsType::Ubuntu => 11,
        OsType::Debian => 12,
        OsType::Fedora => 13,
        OsType::Arch => 14,
    }
}

fn virtual_device_type_code(device_type: VirtualDeviceType) -> u32 {
    match device_type {
        VirtualDeviceType::DvdRom => 1,
        VirtualDeviceType::HardDisk => 2,
        VirtualDeviceType::UsbMassStorage => 3,
    }
}

fn push_u16(data: &mut Vec<u8>, value: u16) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_extent_record(
    data: &mut Vec<u8>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    push_u64(data, virtual_block_start);
    push_u64(data, physical_lba);
    push_u64(data, block_count);
}

unsafe fn load_file_path_from_device_path(file_path: *const FfiDevicePath) -> Option<String> {
    if file_path.is_null() {
        return None;
    }

    let mut node = file_path.cast::<u8>();
    let mut path = String::new();

    for _ in 0..64 {
        let node_type = unsafe { *node };
        let node_subtype = unsafe { *node.add(1) };
        let length = u16::from_le_bytes([unsafe { *node.add(2) }, unsafe { *node.add(3) }]);
        let length = usize::from(length);

        if length < 4 {
            return None;
        }

        if node_type == 0x7f {
            break;
        }

        if node_type == 0x04 && node_subtype == 0x04 {
            let units = (length - 4) / 2;
            let chars = unsafe { node.add(4).cast::<u16>() };

            for index in 0..units {
                let unit = unsafe { ptr::read_unaligned(chars.add(index)) };
                if unit == 0 {
                    break;
                }

                let ch = char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}');
                path.push(if ch == '\\' { '/' } else { ch });
            }
        }

        node = unsafe { node.add(length) };
    }

    if path.is_empty() {
        None
    } else {
        Some(normalize_load_file_key(&path))
    }
}

/// 引导模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// 直接内核启动
    DirectKernel,
    /// EFI 链式加载
    ChainLoad,
    /// 虚拟设备引导
    VirtualDevice,
    /// 内存引导
    MemDisk,
}

impl Default for BootMode {
    fn default() -> Self {
        BootMode::VirtualDevice
    }
}

/// 引导选项
#[derive(Debug, Clone)]
pub struct BootOptions {
    /// 引导模式
    pub mode: BootMode,
    /// 内核参数
    pub kernel_args: String,
    /// 是否启用调试
    pub debug: bool,
    /// 超时 (秒)
    pub timeout: Option<u64>,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            mode: BootMode::default(),
            kernel_args: String::new(),
            debug: false,
            timeout: None,
        }
    }
}

/// 内存映射信息
#[derive(Debug, Clone)]
pub struct MemoryMapInfo {
    /// 起始地址
    pub start: u64,
    /// 大小
    pub size: u64,
    /// 类型
    pub memory_type: u32,
}

/// 分配引导内存
pub fn allocate_boot_memory(bt: &BootServices, size: usize) -> uefi::Result<*mut u8> {
    use uefi::table::boot::MemoryType;

    let pages = (size + 4095) / 4096;

    bt.allocate_pages(
        uefi::table::boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        pages,
    )
    .map(|addr| addr as *mut u8)
}

/// 释放引导内存
pub fn free_boot_memory(bt: &BootServices, ptr: *mut u8, size: usize) -> uefi::Result<()> {
    let pages = (size + 4095) / 4096;
    unsafe { bt.free_pages(ptr as u64, pages) }
}
