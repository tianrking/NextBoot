//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::init::StorageDevice;
use crate::scanner::{IsoFile, OsType};
use crate::virtual_fs::{IsoSimpleFileSystemProtocol, RegisteredIsoSimpleFileSystem};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::{self, NonNull};
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::{BlockIoOps, FileSystem, FsError};
use nextboot_virtio::protocol::append_file_path_device_path;
use nextboot_virtio::{
    PhysicalReader, VirtIoError, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType,
};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::proto::unsafe_protocol;
use uefi::table::boot::{BootServices, LoadImageSource};
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

    fn chain_load_path(&self, device: &VirtualBootDevice, path: &str) -> uefi::Result<()> {
        let data = self.load_file(path)?;
        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        self.chain_load(device, path, &data)
    }

    /// 链式加载 EFI 文件
    fn chain_load(&self, device: &VirtualBootDevice, path: &str, data: &[u8]) -> uefi::Result<()> {
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

    /// 从 ISO 加载文件
    fn load_file(&self, path: &str) -> uefi::Result<Vec<u8>> {
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

        let simple_file_system =
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
        push_u64(&mut data, self.iso.size);
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
            let block_count = div_round_up(self.iso.size, u64::from(self.iso.block_size))
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

        let device_type = match self.iso.os_type {
            OsType::Windows | OsType::WinPE => VirtualDeviceType::DvdRom,
            _ => VirtualDeviceType::HardDisk,
        };
        let virtual_block_size = match device_type {
            VirtualDeviceType::DvdRom => 2048,
            _ => self.iso.block_size,
        };

        let mut config = VirtualDeviceConfig::new(
            device_type,
            self.iso.start_lba,
            self.iso.size,
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
        } else if matches!(device_type, VirtualDeviceType::DvdRom) {
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
        let mut vbio = if self.iso.extents.is_empty() {
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

        let mut last_status = Status::NOT_FOUND;
        for path in default_efi_boot_paths() {
            let full_path = append_file_path_device_path(&device.device_path, path)
                .ok_or(Status::INVALID_PARAMETER)?;
            let device_path =
                unsafe { DevicePath::from_ffi_ptr(full_path.as_ptr().cast::<FfiDevicePath>()) };

            info!("Trying virtual EFI loader path: {}", path);
            match self.bt.load_image(
                self.parent_image,
                LoadImageSource::FromDevicePath {
                    device_path,
                    from_boot_manager: true,
                },
            ) {
                Ok(image) => {
                    info!("Loaded EFI image {:?} from virtual device", image);
                    match self.bt.start_image(image) {
                        Ok(()) => return Ok(()),
                        Err(err) => {
                            last_status = err.status();
                            warn!("StartImage failed for {} with {:?}", path, err.status());
                            let _ = self.bt.unload_image(image);
                        }
                    }
                }
                Err(err) => {
                    last_status = err.status();
                    warn!("LoadImage failed for {} with {:?}", path, err.status());
                }
            }
        }

        Err(last_status.into())
    }
}

struct VirtualBootDevice {
    handle: Handle,
    device_path: Vec<u8>,
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
