//! UEFI Block IO Protocol 实现
//!
//! 定义与 UEFI 兼容的协议接口

use crate::{VirtualBlockIo, VirtualMediaInfo};
#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use core::ffi::c_void;
#[cfg(not(test))]
use uefi::proto::device_path::DevicePath;
#[cfg(not(test))]
use uefi::proto::media::block::BlockIO;
#[cfg(not(test))]
use uefi::proto::unsafe_protocol;
#[cfg(not(test))]
use uefi::table::boot::BootServices;
#[cfg(not(test))]
use uefi::{Handle, Identify};

mod device_path;
mod handlers;
mod types;

#[cfg(test)]
use device_path::NEXTBOOT_VIRTUAL_DISK_GUID;

pub use device_path::{
    append_file_path_device_path, create_cdrom_device_path, create_hard_drive_device_path,
    create_virtual_disk_controller_device_path, format_guid, CdRomDevicePath, DevicePathHeader,
    DevicePathType, EndDevicePath, FilePathDevicePath, HardDriveDevicePath, HardwareSubtype,
    MediaSubtype, MessagingSubtype, VendorHardwareDevicePath,
};
pub use types::{
    BlockIo2Protocol, BlockIo2Token, BlockIoMedia, BlockIoProtocol, DiskIo2Protocol, DiskIo2Token,
    DiskIoProtocol, UefiStatus, BLOCK_IO_2_GUID, BLOCK_IO_GUID, DEVICE_PATH_GUID, DISK_IO_2_GUID,
    DISK_IO_GUID, LOADED_IMAGE_GUID,
};

#[cfg(not(test))]
#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("a77b2472-e282-4e9f-a245-c2c0e27bbcc1")]
struct BlockIo2(BlockIo2Protocol);

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
        let mut device_path = protocol.device_path_bytes().into_boxed_slice();
        let interfaces = [
            (BlockIO::GUID, protocol.as_ptr().cast::<c_void>()),
            (BlockIo2::GUID, protocol.block_io_2_ptr().cast::<c_void>()),
            (DevicePath::GUID, device_path.as_mut_ptr().cast::<c_void>()),
        ];
        // Let firmware's DiskIo driver adapt BlockIO when ConnectController runs.
        // Preinstalling DiskIo here conflicts with that driver's Start/Stop ownership
        // and can leave a dangling BY_DRIVER BlockIo2 open after its failed Start.
        let mut installed_handle = None;
        for (index, (guid, interface)) in interfaces.iter().enumerate() {
            match unsafe { bt.install_protocol_interface(installed_handle, guid, *interface) } {
                Ok(handle) => installed_handle = Some(handle),
                Err(error) => {
                    let mut rollback_ok = true;
                    if let Some(handle) = installed_handle {
                        for (guid, interface) in interfaces[..index].iter().rev() {
                            rollback_ok &= unsafe {
                                bt.uninstall_protocol_interface(handle, guid, *interface)
                            }.is_ok();
                        }
                    }
                    if !rollback_ok {
                        let _ = Box::leak(protocol);
                        let _ = Box::leak(device_path);
                        return Err(uefi::Status::COMPROMISED_DATA.into());
                    }
                    return Err(error);
                }
            }
        }
        let handle = installed_handle.ok_or(uefi::Status::DEVICE_ERROR)?;
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

    /// Unregister interfaces before releasing their backing allocations.
    /// On failure keep the allocations alive because firmware may still refer to them.
    pub fn uninstall(mut self, bt: &BootServices) -> uefi::Result<()> {
        let interfaces = [
            (DevicePath::GUID, self.device_path_ptr().cast::<c_void>()),
            (BlockIo2::GUID, self.block_io_2_ptr().cast::<c_void>()),
            (BlockIO::GUID, self.protocol_ptr().cast::<c_void>()),
        ];
        let mut failure = None;
        for (guid, interface) in interfaces {
            if let Err(error) = unsafe {
                bt.uninstall_protocol_interface(self.handle, &guid, interface)
            } {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            self.leak();
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
