//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::init::StorageDevice;
use crate::scanner::{IsoFile, OsType};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::NonNull;
use log::{info, warn};
use nextboot_virtio::protocol::append_file_path_device_path;
use nextboot_virtio::{PhysicalReader, VirtIoError};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::{BootServices, LoadImageSource};
use uefi::{Handle, Status};

const DEFAULT_EFI_BOOT_PATHS: &[&str] = &[
    "\\EFI\\BOOT\\BOOTX64.EFI",
    "\\EFI\\BOOT\\BOOTAA64.EFI",
    "\\EFI\\BOOT\\BOOTIA32.EFI",
    "\\EFI\\BOOT\\BOOTARM.EFI",
];

/// 引导管理器
pub struct BootManager<'a> {
    bt: &'a BootServices,
    parent_image: Handle,
    device: &'a StorageDevice,
    iso: &'a IsoFile,
}

impl<'a> BootManager<'a> {
    /// 创建新的引导管理器
    pub fn new(
        bt: &'a BootServices,
        parent_image: Handle,
        device: &'a StorageDevice,
        iso: &'a IsoFile,
    ) -> Self {
        Self {
            bt,
            parent_image,
            device,
            iso,
        }
    }

    /// 准备并执行引导
    pub fn prepare_and_boot(&self) -> uefi::Result<()> {
        info!("Preparing to boot: {}", self.iso.path);
        let virtual_device = self.create_virtual_block_io()?;

        match self.boot_virtual_device(&virtual_device) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "Virtual device boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                );

                match self.iso.os_type {
                    OsType::Windows | OsType::WinPE => self.boot_windows(),
                    OsType::Ubuntu
                    | OsType::Debian
                    | OsType::Fedora
                    | OsType::Arch
                    | OsType::Linux => self.boot_linux(),
                    OsType::Unknown => self.boot_generic(),
                }
            }
        }
    }

    /// 引导 Linux ISO
    fn boot_linux(&self) -> uefi::Result<()> {
        use nextboot_linux::{LinuxBootConfig, LinuxBootloader, LinuxDistro};

        info!("Booting Linux ISO...");

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

        // 执行引导
        info!("Starting Linux kernel...");
        unsafe {
            bootloader.boot();
        }

        // 不会到达这里
        Ok(())
    }

    /// 引导 Windows ISO
    fn boot_windows(&self) -> uefi::Result<()> {
        use nextboot_windows::{WindowsBootConfig, WindowsBootloader};

        info!("Booting Windows ISO...");

        // 创建启动配置
        let config = WindowsBootConfig::new();

        info!("Boot file: {}", config.bootmgfw_path);

        // 创建启动器
        let mut bootloader = WindowsBootloader::new(config);

        // 准备启动环境
        bootloader.prepare().map_err(|_| Status::DEVICE_ERROR)?;

        // 加载 bootmgfw.efi
        let bootmgfw_data = self.load_file(&bootloader.config().bootmgfw_path)?;
        bootloader
            .load_bootmgfw(bootmgfw_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        // 执行引导
        info!("Starting Windows Boot Manager...");
        unsafe {
            bootloader.boot();
        }

        // 不会到达这里
        Ok(())
    }

    /// 通用引导 (尝试链式加载)
    fn boot_generic(&self) -> uefi::Result<()> {
        info!("Attempting generic boot...");

        // 检查 ISO 中的 EFI 引导文件
        let efi_boot_paths = [
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
            "/boot/efi/bootx64.efi",
        ];

        for path in &efi_boot_paths {
            if let Ok(data) = self.load_file(path) {
                if !data.is_empty() {
                    info!("Found EFI boot file: {}", path);
                    return self.chain_load(path, &data);
                }
            }
        }

        // 尝试 Linux 方式
        warn!("No EFI boot file found, trying Linux boot method");
        self.boot_linux()
    }

    /// 链式加载 EFI 文件
    fn chain_load(&self, path: &str, data: &[u8]) -> uefi::Result<()> {
        info!("Chain loading: {}", path);

        // TODO: 实现链式加载
        // 1. 分配内存
        // 2. 复制 EFI 镜像
        // 3. 调用 LoadImage
        // 4. 调用 StartImage

        Err(Status::UNSUPPORTED.into())
    }

    /// 从 ISO 加载文件
    fn load_file(&self, path: &str) -> uefi::Result<Vec<u8>> {
        info!("Loading file: {}", path);

        // TODO: 实现文件加载
        // 1. 使用文件系统驱动打开 ISO
        // 2. 定位文件
        // 3. 读取文件内容

        // 简化实现：返回空数据
        Ok(Vec::new())
    }

    /// 创建虚拟 Block IO
    fn create_virtual_block_io(&self) -> uefi::Result<VirtualBootDevice> {
        use nextboot_virtio::protocol::VirtualBlockIoProtocol;
        use nextboot_virtio::{
            CdRomBootInfo, VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType,
        };

        info!("Creating virtual Block IO...");

        // 确定设备类型
        let device_type = match self.iso.os_type {
            OsType::Windows | OsType::WinPE => VirtualDeviceType::DvdRom,
            _ => VirtualDeviceType::HardDisk,
        };
        let virtual_block_size = match device_type {
            VirtualDeviceType::DvdRom => 2048,
            _ => self.iso.block_size,
        };

        // 创建配置
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

        // 创建虚拟 Block IO
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

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = UefiPhysicalReader::new(&source_block_io).ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);

        let virtual_info = vbio.device_info();
        let registered = VirtualBlockIoProtocol::new(vbio).install(self.bt)?;
        let virtual_handle = registered.handle();
        let device_path = registered.device_path().to_vec();
        registered.leak();

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

    fn boot_virtual_device(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Connecting virtual boot device {:?}", device.handle);
        if let Err(err) = self.bt.connect_controller(device.handle, None, None, true) {
            warn!(
                "ConnectController on virtual device returned {:?}; trying LoadImage anyway",
                err.status()
            );
        }

        let mut last_status = Status::NOT_FOUND;
        for path in DEFAULT_EFI_BOOT_PATHS {
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
