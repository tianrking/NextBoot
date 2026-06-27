//! UEFI Block IO Protocol 实现
//!
//! 定义与 UEFI 兼容的协议接口

use crate::{VirtIoError, VirtualBlockIo, VirtualMediaInfo};
#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use core::ffi::c_void;
#[cfg(not(test))]
use uefi::proto::device_path::DevicePath;
#[cfg(not(test))]
use uefi::proto::media::block::BlockIO;
#[cfg(not(test))]
use uefi::proto::media::disk::{DiskIo, DiskIo2};
#[cfg(not(test))]
use uefi::proto::unsafe_protocol;
#[cfg(not(test))]
use uefi::table::boot::BootServices;
#[cfg(not(test))]
use uefi::{Handle, Identify};

mod device_path;
mod handlers;

#[cfg(test)]
use device_path::NEXTBOOT_VIRTUAL_DISK_GUID;

pub use device_path::{
    append_file_path_device_path, create_cdrom_device_path, create_hard_drive_device_path,
    create_virtual_disk_controller_device_path, format_guid, CdRomDevicePath, DevicePathHeader,
    DevicePathType, EndDevicePath, FilePathDevicePath, HardDriveDevicePath, HardwareSubtype,
    MediaSubtype, MessagingSubtype, VendorHardwareDevicePath,
};

#[cfg(not(test))]
#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("a77b2472-e282-4e9f-a245-c2c0e27bbcc1")]
struct BlockIo2(BlockIo2Protocol);

/// UEFI Block IO Protocol GUID
pub const BLOCK_IO_GUID: [u8; 16] = [
    0x21, 0x5b, 0x4e, 0x96, 0x59, 0x64, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// UEFI Block IO 2 Protocol GUID
pub const BLOCK_IO_2_GUID: [u8; 16] = [
    0x72, 0x24, 0x7b, 0xa7, 0x82, 0xe2, 0x9f, 0x4e, 0xa2, 0x45, 0xc2, 0xc0, 0xe2, 0x7b, 0xbc, 0xc1,
];

/// UEFI Disk IO Protocol GUID
pub const DISK_IO_GUID: [u8; 16] = [
    0x71, 0x51, 0x34, 0xce, 0x0b, 0xba, 0xd2, 0x11, 0x8e, 0x4f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// UEFI Disk IO 2 Protocol GUID
pub const DISK_IO_2_GUID: [u8; 16] = [
    0xae, 0x8e, 0x1c, 0x15, 0x2c, 0x7f, 0x2c, 0x47, 0x9e, 0x54, 0x98, 0x28, 0x19, 0x4f, 0x6a, 0x88,
];

/// 设备路径协议 GUID
pub const DEVICE_PATH_GUID: [u8; 16] = [
    0x91, 0x6e, 0x57, 0x09, 0x3f, 0x6d, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// 加载的镜像协议 GUID
pub const LOADED_IMAGE_GUID: [u8; 16] = [
    0xa1, 0x51, 0xbc, 0x5c, 0x16, 0x4a, 0x76, 0x4d, 0x87, 0x2c, 0x3a, 0x4e, 0xaa, 0x9b, 0x66, 0x53,
];

/// Block IO Protocol 结构体 (UEFI 定义)
#[repr(C)]
pub struct BlockIoProtocol {
    pub revision: u64,
    pub media: *const BlockIoMedia,
    pub reset: extern "efiapi" fn(*mut BlockIoProtocol, bool) -> u64,
    pub read_blocks:
        extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *mut core::ffi::c_void) -> u64,
    pub write_blocks:
        extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *const core::ffi::c_void) -> u64,
    pub flush_blocks: extern "efiapi" fn(*mut BlockIoProtocol) -> u64,
}

/// Block IO 2 Protocol (异步版本)
#[derive(Debug)]
#[repr(C)]
pub struct BlockIo2Protocol {
    pub media: *const BlockIoMedia,
    pub reset: extern "efiapi" fn(*mut BlockIo2Protocol, bool) -> u64,
    pub read_blocks_ex: extern "efiapi" fn(
        *mut BlockIo2Protocol,
        u32,
        u64,
        *mut BlockIo2Token,
        usize,
        *mut core::ffi::c_void,
    ) -> u64,
    pub write_blocks_ex: extern "efiapi" fn(
        *mut BlockIo2Protocol,
        u32,
        u64,
        *mut BlockIo2Token,
        usize,
        *const core::ffi::c_void,
    ) -> u64,
    pub flush_blocks_ex: extern "efiapi" fn(*mut BlockIo2Protocol, *mut BlockIo2Token) -> u64,
}

/// Block IO 2 Token (异步操作)
#[repr(C)]
pub struct BlockIo2Token {
    pub event: *mut core::ffi::c_void,
    pub transaction_status: u64,
}

/// Block IO 媒体信息
#[repr(C)]
pub struct BlockIoMedia {
    pub media_id: u32,
    pub removable_media: bool,
    pub media_present: bool,
    pub logical_partition: bool,
    pub read_only: bool,
    pub write_caching: bool,
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
    // UEFI 2.7+ 扩展
    pub lowest_aligned_lba: u64,
    pub logical_blocks_per_physical_block: u32,
    pub optimal_transfer_length_granularity: u32,
}

/// Disk IO Protocol 结构体 (UEFI 定义)
#[repr(C)]
pub struct DiskIoProtocol {
    pub revision: u64,
    pub read_disk:
        extern "efiapi" fn(*mut DiskIoProtocol, u32, u64, usize, *mut core::ffi::c_void) -> u64,
    pub write_disk:
        extern "efiapi" fn(*mut DiskIoProtocol, u32, u64, usize, *const core::ffi::c_void) -> u64,
}

/// Disk IO 2 Protocol 结构体 (UEFI 定义)
#[repr(C)]
pub struct DiskIo2Protocol {
    pub revision: u64,
    pub cancel: extern "efiapi" fn(*mut DiskIo2Protocol) -> u64,
    pub read_disk_ex: extern "efiapi" fn(
        *mut DiskIo2Protocol,
        u32,
        u64,
        *mut DiskIo2Token,
        usize,
        *mut core::ffi::c_void,
    ) -> u64,
    pub write_disk_ex: extern "efiapi" fn(
        *mut DiskIo2Protocol,
        u32,
        u64,
        *mut DiskIo2Token,
        usize,
        *const core::ffi::c_void,
    ) -> u64,
    pub flush_disk_ex: extern "efiapi" fn(*mut DiskIo2Protocol, *mut DiskIo2Token) -> u64,
}

/// Disk IO 2 Token (异步操作)
#[repr(C)]
pub struct DiskIo2Token {
    pub event: *mut core::ffi::c_void,
    pub transaction_status: u64,
}

impl BlockIoMedia {
    /// 从虚拟媒体信息创建
    pub fn from_virtual_info(info: &VirtualMediaInfo) -> Self {
        Self {
            media_id: info.media_id,
            removable_media: info.flags.contains(crate::MediaFlags::REMOVABLE),
            media_present: info.flags.contains(crate::MediaFlags::MEDIA_PRESENT),
            logical_partition: false,
            read_only: info.flags.contains(crate::MediaFlags::READ_ONLY),
            write_caching: false,
            block_size: info.block_size,
            io_align: 8,
            last_block: info.last_block,
            lowest_aligned_lba: 0,
            logical_blocks_per_physical_block: 1,
            optimal_transfer_length_granularity: 1,
        }
    }
}

/// UEFI 状态码
#[derive(Debug, Clone, Copy)]
#[repr(u64)]
pub enum UefiStatus {
    Success = 0,
    InvalidParameter = 0x8000000000000002,
    Unsupported = 0x8000000000000003,
    BadBufferSize = 0x8000000000000004,
    BufferTooSmall = 0x8000000000000005,
    NotReady = 0x8000000000000006,
    DeviceError = 0x8000000000000007,
    WriteProtected = 0x8000000000000008,
    OutOfResources = 0x8000000000000009,
    MediaChanged = 0x8000000000000016,
    NoMedia = 0x800000000000001E,
}

impl From<VirtIoError> for UefiStatus {
    fn from(err: VirtIoError) -> Self {
        match err {
            VirtIoError::OutOfBounds => UefiStatus::InvalidParameter,
            VirtIoError::WriteProtected => UefiStatus::WriteProtected,
            VirtIoError::ReadFailed => UefiStatus::DeviceError,
            VirtIoError::InvalidArgument => UefiStatus::InvalidParameter,
            VirtIoError::InvalidBufferSize => UefiStatus::BadBufferSize,
            VirtIoError::MediaChanged => UefiStatus::MediaChanged,
            VirtIoError::InvalidMapping => UefiStatus::DeviceError,
            VirtIoError::NoPhysicalRead => UefiStatus::NotReady,
            VirtIoError::DeviceError => UefiStatus::DeviceError,
            VirtIoError::CrcError => UefiStatus::DeviceError,
        }
    }
}

/// 虚拟 Block IO 协议包装器
#[repr(C)]
pub struct VirtualBlockIoProtocol {
    protocol: BlockIoProtocol,
    block_io_2: BlockIo2Protocol,
    disk_io: DiskIoProtocol,
    disk_io_2: DiskIo2Protocol,
    media: BlockIoMedia,
    vbio: core::cell::UnsafeCell<VirtualBlockIo>,
    boot_services: *const core::ffi::c_void,
}

impl VirtualBlockIoProtocol {
    /// 创建新的虚拟 Block IO 协议
    pub fn new(vbio: VirtualBlockIo) -> Self {
        let media = BlockIoMedia::from_virtual_info(&VirtualMediaInfo::from_config(vbio.config()));

        Self {
            protocol: BlockIoProtocol {
                revision: 0x00010000, // Revision 1.0
                media: core::ptr::null(),
                reset: Self::reset_handler,
                read_blocks: Self::read_blocks_handler,
                write_blocks: Self::write_blocks_handler,
                flush_blocks: Self::flush_handler,
            },
            block_io_2: BlockIo2Protocol {
                media: core::ptr::null(),
                reset: Self::reset_2_handler,
                read_blocks_ex: Self::read_blocks_ex_handler,
                write_blocks_ex: Self::write_blocks_ex_handler,
                flush_blocks_ex: Self::flush_blocks_ex_handler,
            },
            disk_io: DiskIoProtocol {
                revision: 0x00010000,
                read_disk: Self::read_disk_handler,
                write_disk: Self::write_disk_handler,
            },
            disk_io_2: DiskIo2Protocol {
                revision: 0x00020000,
                cancel: Self::cancel_disk_ex_handler,
                read_disk_ex: Self::read_disk_ex_handler,
                write_disk_ex: Self::write_disk_ex_handler,
                flush_disk_ex: Self::flush_disk_ex_handler,
            },
            media,
            vbio: core::cell::UnsafeCell::new(vbio),
            boot_services: core::ptr::null(),
        }
    }

    /// 获取协议指针
    pub fn as_ptr(&mut self) -> *mut BlockIoProtocol {
        self.protocol.media = &self.media;
        self.block_io_2.media = &self.media;
        &mut self.protocol
    }

    /// 获取 Block IO 2 协议指针
    pub fn block_io_2_ptr(&mut self) -> *mut BlockIo2Protocol {
        self.block_io_2.media = &self.media;
        &mut self.block_io_2
    }

    /// 获取 Disk IO 协议指针
    pub fn disk_io_ptr(&mut self) -> *mut DiskIoProtocol {
        &mut self.disk_io
    }

    /// 获取 Disk IO 2 协议指针
    pub fn disk_io_2_ptr(&mut self) -> *mut DiskIo2Protocol {
        &mut self.disk_io_2
    }

    /// 安装为 UEFI Block IO 协议。
    #[cfg(not(test))]
    pub fn install(self, bt: &BootServices) -> uefi::Result<RegisteredVirtualBlockIo> {
        let mut protocol = Box::new(self);
        protocol.boot_services = bt as *const BootServices as *const c_void;
        let block_io_interface = protocol.as_ptr().cast::<c_void>();
        let handle =
            unsafe { bt.install_protocol_interface(None, &BlockIO::GUID, block_io_interface) }?;

        let block_io_2_interface = protocol.block_io_2_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &BlockIo2::GUID, block_io_2_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIO::GUID, block_io_interface)
            };
            return Err(err);
        }

        let disk_io_interface = protocol.disk_io_ptr().cast::<c_void>();
        if let Err(err) =
            unsafe { bt.install_protocol_interface(Some(handle), &DiskIo::GUID, disk_io_interface) }
        {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIo2::GUID, block_io_2_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIO::GUID, block_io_interface)
            };
            return Err(err);
        }

        let disk_io_2_interface = protocol.disk_io_2_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &DiskIo2::GUID, disk_io_2_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &DiskIo::GUID, disk_io_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIo2::GUID, block_io_2_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIO::GUID, block_io_interface)
            };
            return Err(err);
        }

        let mut device_path = protocol.device_path_bytes().into_boxed_slice();
        let device_path_interface = device_path.as_mut_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &DevicePath::GUID, device_path_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &DiskIo2::GUID, disk_io_2_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &DiskIo::GUID, disk_io_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIo2::GUID, block_io_2_interface)
            };
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &BlockIO::GUID, block_io_interface)
            };
            return Err(err);
        }

        Ok(RegisteredVirtualBlockIo {
            handle,
            protocol,
            device_path,
        })
    }

    #[cfg(not(test))]
    fn device_path_bytes(&self) -> alloc::vec::Vec<u8> {
        let vbio = unsafe { &*self.vbio.get() };
        let info = vbio.device_info();

        match info.device_type {
            crate::VirtualDeviceType::DvdRom => {
                if let Some(boot) = vbio.config().cdrom_boot {
                    create_cdrom_device_path(
                        boot.boot_entry,
                        boot.image_lba,
                        boot.image_block_count,
                    )
                } else {
                    create_cdrom_device_path(0, 0, info.block_count)
                }
            }
            crate::VirtualDeviceType::HardDisk | crate::VirtualDeviceType::UsbMassStorage => {
                create_virtual_disk_controller_device_path()
            }
        }
    }
}

/// 已注册的虚拟 Block IO。
///
/// 持有协议对象的 Box，确保 firmware 保存的协议指针在 boot-services
/// 生命周期内不会悬空。
#[cfg(not(test))]
pub struct RegisteredVirtualBlockIo {
    handle: Handle,
    protocol: Box<VirtualBlockIoProtocol>,
    device_path: Box<[u8]>,
}

#[cfg(not(test))]
impl RegisteredVirtualBlockIo {
    pub fn handle(&self) -> Handle {
        self.handle
    }

    pub fn device_path(&self) -> &[u8] {
        &self.device_path
    }

    pub fn protocol_ptr(&mut self) -> *mut BlockIoProtocol {
        self.protocol.as_ptr()
    }

    pub fn block_io_2_ptr(&mut self) -> *mut BlockIo2Protocol {
        self.protocol.block_io_2_ptr()
    }

    pub fn disk_io_ptr(&mut self) -> *mut DiskIoProtocol {
        self.protocol.disk_io_ptr()
    }

    pub fn disk_io_2_ptr(&mut self) -> *mut DiskIo2Protocol {
        self.protocol.disk_io_2_ptr()
    }

    pub fn device_path_ptr(&mut self) -> *mut u8 {
        self.device_path.as_mut_ptr()
    }

    pub fn leak(self) -> Handle {
        let handle = self.handle;
        let protocol = self.protocol;
        let device_path = self.device_path;
        let _ = Box::leak(protocol);
        let _ = Box::leak(device_path);
        handle
    }
}

#[cfg(test)]
mod tests;
