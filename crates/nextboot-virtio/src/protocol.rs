//! UEFI Block IO Protocol 实现
//!
//! 定义与 UEFI 兼容的协议接口

use crate::{VirtIoError, VirtualBlockIo, VirtualMediaInfo};
#[cfg(not(test))]
use alloc::boxed::Box;
use bitflags::bitflags;
#[cfg(not(test))]
use core::ffi::c_void;
#[cfg(not(test))]
use uefi::proto::device_path::DevicePath;
#[cfg(not(test))]
use uefi::proto::media::block::BlockIO;
#[cfg(not(test))]
use uefi::proto::media::disk::DiskIo;
#[cfg(not(test))]
use uefi::table::boot::BootServices;
#[cfg(not(test))]
use uefi::{Handle, Identify};

/// UEFI Block IO Protocol GUID
pub const BLOCK_IO_GUID: [u8; 16] = [
    0x21, 0x5b, 0x4e, 0x96, 0x59, 0x64, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// UEFI Block IO 2 Protocol GUID
pub const BLOCK_IO_2_GUID: [u8; 16] = [
    0xa8, 0x63, 0x9a, 0x14, 0x5d, 0x37, 0x7b, 0x4e, 0xa9, 0x88, 0x6c, 0x42, 0xf4, 0x3e, 0x4c, 0x9a,
];

/// UEFI Disk IO Protocol GUID
pub const DISK_IO_GUID: [u8; 16] = [
    0x71, 0x51, 0x34, 0xce, 0x0b, 0xba, 0xd2, 0x11, 0x8e, 0x4f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// 设备路径协议 GUID
pub const DEVICE_PATH_GUID: [u8; 16] = [
    0x91, 0x6e, 0x57, 0x09, 0x3f, 0x6d, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b,
];

/// 加载的镜像协议 GUID
pub const LOADED_IMAGE_GUID: [u8; 16] = [
    0xa1, 0x51, 0xbc, 0x5c, 0x16, 0x4a, 0x76, 0x4d, 0x87, 0x2c, 0x3a, 0x4e, 0xaa, 0x9b, 0x66, 0x53,
];

bitflags! {
    /// 设备路径类型
    #[derive(Debug, Clone, Copy)]
    pub struct DevicePathType: u8 {
        const HARDWARE = 0x01;
        const ACPI = 0x02;
        const MESSAGING = 0x03;
        const MEDIA = 0x04;
        const END = 0x7F;
    }
}

/// 媒体设备路径子类型
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum MediaSubtype {
    /// 硬盘
    HardDrive = 0x01,
    /// CD-ROM
    CdRom = 0x02,
    /// 供应商特定
    Vendor = 0x03,
    /// 文件路径
    FilePath = 0x04,
    /// 媒体协议
    Protocol = 0x05,
    /// PIWG 固件文件
    PiwgFirmwareFile = 0x06,
    /// PIWG 固件卷
    PiwgFirmwareVolume = 0x07,
}

/// 消息设备路径子类型
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum MessagingSubtype {
    /// ATAPI
    Atapi = 0x01,
    /// SCSI
    Scsi = 0x02,
    /// Fibre Channel
    FibreChannel = 0x03,
    /// 1394
    I394 = 0x04,
    /// USB
    Usb = 0x05,
    /// USB 类
    UsbClass = 0x0F,
    /// SATA
    Sata = 0x12,
}

/// 设备路径节点头
#[repr(C, packed)]
pub struct DevicePathHeader {
    pub type_: u8,
    pub subtype: u8,
    pub length: [u8; 2], // Little-endian u16
}

impl DevicePathHeader {
    /// 创建新的设备路径头
    pub fn new(type_: DevicePathType, subtype: u8, length: u16) -> Self {
        Self {
            type_: type_.bits(),
            subtype,
            length: length.to_le_bytes(),
        }
    }

    /// 获取长度
    pub fn get_length(&self) -> u16 {
        u16::from_le_bytes(self.length)
    }
}

/// 硬盘设备路径
#[repr(C, packed)]
pub struct HardDriveDevicePath {
    pub header: DevicePathHeader,
    pub partition_number: u32,
    pub partition_start: u64,
    pub partition_size: u64,
    pub partition_signature: [u8; 16],
    pub partition_format: u8,
    pub signature_type: u8,
}

impl HardDriveDevicePath {
    /// 创建新的硬盘设备路径
    pub fn new(partition_number: u32, start: u64, size: u64) -> Self {
        Self {
            header: DevicePathHeader::new(DevicePathType::MEDIA, MediaSubtype::HardDrive as u8, 42),
            partition_number,
            partition_start: start,
            partition_size: size,
            partition_signature: [0u8; 16],
            partition_format: 0x02, // GPT
            signature_type: 0x02,   // GUID
        }
    }
}

/// CD-ROM 设备路径
#[repr(C, packed)]
pub struct CdRomDevicePath {
    pub header: DevicePathHeader,
    pub boot_entry: u32,
    pub partition_start: u64,
    pub partition_size: u64,
}

impl CdRomDevicePath {
    /// 创建新的 CD-ROM 设备路径
    pub fn new(boot_entry: u32, start: u64, size: u64) -> Self {
        Self {
            header: DevicePathHeader::new(DevicePathType::MEDIA, MediaSubtype::CdRom as u8, 24),
            boot_entry,
            partition_start: start,
            partition_size: size,
        }
    }
}

/// 文件路径设备路径
#[repr(C)]
pub struct FilePathDevicePath {
    pub header: DevicePathHeader,
    pub path: [u16; 1], // 变长，以 null 结尾的 UTF-16 字符串
}

impl FilePathDevicePath {
    /// 创建文件路径设备路径
    pub fn new(path: &[u16]) -> alloc::vec::Vec<u8> {
        let header_size = 4u16;
        let path_size = (path.len() + 1) * 2; // +1 for null terminator
        let total_size = header_size as usize + path_size;

        let mut data = alloc::vec![0u8; total_size];

        // 写入头
        data[0] = DevicePathType::MEDIA.bits();
        data[1] = MediaSubtype::FilePath as u8;
        let len_bytes = (total_size as u16).to_le_bytes();
        data[2] = len_bytes[0];
        data[3] = len_bytes[1];

        // 写入路径 (UTF-16LE)
        for (i, &c) in path.iter().enumerate() {
            let offset = 4 + i * 2;
            let bytes = c.to_le_bytes();
            data[offset] = bytes[0];
            data[offset + 1] = bytes[1];
        }

        // null 终止符
        let null_offset = 4 + path.len() * 2;
        data[null_offset] = 0;
        data[null_offset + 1] = 0;

        data
    }
}

fn normalize_uefi_file_path(path: &str) -> alloc::vec::Vec<u16> {
    let mut out = alloc::vec::Vec::new();

    if !path.starts_with('\\') && !path.starts_with('/') {
        out.push('\\' as u16);
    }

    for ch in path.chars() {
        let ch = if ch == '/' { '\\' } else { ch };
        let mut buf = [0u16; 2];
        out.extend_from_slice(ch.encode_utf16(&mut buf));
    }

    out
}

/// 结束设备路径
#[repr(C, packed)]
pub struct EndDevicePath {
    pub header: DevicePathHeader,
}

impl EndDevicePath {
    /// 创建结束设备路径
    pub fn new() -> Self {
        Self {
            header: DevicePathHeader::new(
                DevicePathType::END,
                0xFF, // End Entire
                4,
            ),
        }
    }

    /// 转换为字节
    pub fn to_bytes(&self) -> [u8; 4] {
        [self.header.type_, self.header.subtype, 4, 0]
    }
}

impl Default for EndDevicePath {
    fn default() -> Self {
        Self::new()
    }
}

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
#[repr(C)]
pub struct BlockIo2Protocol {
    pub media: *const BlockIoMedia,
    pub reset: extern "efiapi" fn(*mut BlockIo2Protocol, bool) -> u64,
    pub read_blocks_ex: extern "efiapi" fn(
        *mut BlockIo2Protocol,
        u32,
        u64,
        u64,
        *mut core::ffi::c_void,
        *mut BlockIo2Token,
    ) -> u64,
    pub write_blocks_ex: extern "efiapi" fn(
        *mut BlockIo2Protocol,
        u32,
        u64,
        u64,
        *const core::ffi::c_void,
        *mut BlockIo2Token,
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
    disk_io: DiskIoProtocol,
    media: BlockIoMedia,
    vbio: core::cell::UnsafeCell<VirtualBlockIo>,
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
            disk_io: DiskIoProtocol {
                revision: 0x00010000,
                read_disk: Self::read_disk_handler,
                write_disk: Self::write_disk_handler,
            },
            media,
            vbio: core::cell::UnsafeCell::new(vbio),
        }
    }

    /// 获取协议指针
    pub fn as_ptr(&mut self) -> *mut BlockIoProtocol {
        self.protocol.media = &self.media;
        &mut self.protocol
    }

    /// 获取 Disk IO 协议指针
    pub fn disk_io_ptr(&mut self) -> *mut DiskIoProtocol {
        &mut self.disk_io
    }

    /// 安装为 UEFI Block IO 协议。
    #[cfg(not(test))]
    pub fn install(self, bt: &BootServices) -> uefi::Result<RegisteredVirtualBlockIo> {
        let mut protocol = Box::new(self);
        let block_io_interface = protocol.as_ptr().cast::<c_void>();
        let handle =
            unsafe { bt.install_protocol_interface(None, &BlockIO::GUID, block_io_interface) }?;

        let disk_io_interface = protocol.disk_io_ptr().cast::<c_void>();
        if let Err(err) =
            unsafe { bt.install_protocol_interface(Some(handle), &DiskIo::GUID, disk_io_interface) }
        {
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
                bt.uninstall_protocol_interface(handle, &DiskIo::GUID, disk_io_interface)
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
                create_hard_drive_device_path(1, 0, info.block_count)
            }
        }
    }

    /// Reset 处理函数
    extern "efiapi" fn reset_handler(this: *mut BlockIoProtocol, extended: bool) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let vbio = unsafe { &*wrapper.vbio.get() };
        vbio.reset(extended)
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    /// ReadBlocks 处理函数
    extern "efiapi" fn read_blocks_handler(
        this: *mut BlockIoProtocol,
        media_id: u32,
        lba: u64,
        buffer_size: u64,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer.is_null() {
            return UefiStatus::InvalidParameter as u64;
        }

        let block_size = wrapper.media.block_size;
        if block_size == 0 || buffer_size % block_size as u64 != 0 {
            return UefiStatus::BadBufferSize as u64;
        }

        if buffer_size > usize::MAX as u64 {
            return UefiStatus::InvalidParameter as u64;
        }

        let buf =
            unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), buffer_size as usize) };
        let vbio = unsafe { &*wrapper.vbio.get() };

        vbio.read_blocks(media_id, lba, buf)
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    /// WriteBlocks 处理函数
    extern "efiapi" fn write_blocks_handler(
        this: *mut BlockIoProtocol,
        media_id: u32,
        lba: u64,
        buffer_size: u64,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer.is_null() {
            return UefiStatus::InvalidParameter as u64;
        }

        if buffer_size > usize::MAX as u64 {
            return UefiStatus::InvalidParameter as u64;
        }

        let buf = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), buffer_size as usize) };
        let vbio = unsafe { &*wrapper.vbio.get() };

        vbio.write_blocks(media_id, lba, buf)
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    /// ReadDisk 处理函数
    extern "efiapi" fn read_disk_handler(
        this: *mut DiskIoProtocol,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer_size == 0 {
            return UefiStatus::Success as u64;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter as u64;
        }

        let buf = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*wrapper.vbio.get() };

        vbio.read_bytes(media_id, offset, buf)
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    /// WriteDisk 处理函数
    extern "efiapi" fn write_disk_handler(
        this: *mut DiskIoProtocol,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer_size == 0 {
            return UefiStatus::Success as u64;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter as u64;
        }

        let buf = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*wrapper.vbio.get() };

        vbio.write_blocks(media_id, offset, buf)
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    /// Flush 处理函数
    extern "efiapi" fn flush_handler(this: *mut BlockIoProtocol) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let vbio = unsafe { &*wrapper.vbio.get() };
        vbio.flush()
            .map(|_| UefiStatus::Success as u64)
            .unwrap_or_else(|err| UefiStatus::from(err) as u64)
    }

    fn from_protocol(this: *mut BlockIoProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        // `protocol` is the first field and the type is repr(C), so both
        // pointers have the same address.
        Some(unsafe { &mut *(this.cast::<Self>()) })
    }

    fn from_disk_protocol(this: *mut DiskIoProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, disk_io);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
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

    pub fn disk_io_ptr(&mut self) -> *mut DiskIoProtocol {
        self.protocol.disk_io_ptr()
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

/// 创建 CD-ROM 设备路径
pub fn create_cdrom_device_path(boot_entry: u32, start: u64, size: u64) -> alloc::vec::Vec<u8> {
    let cdrom = CdRomDevicePath::new(boot_entry, start, size);
    let end = EndDevicePath::new();

    let mut data = alloc::vec::Vec::new();

    // CD-ROM 设备路径
    unsafe {
        let cdrom_bytes = core::slice::from_raw_parts(
            &cdrom as *const CdRomDevicePath as *const u8,
            core::mem::size_of::<CdRomDevicePath>(),
        );
        data.extend_from_slice(cdrom_bytes);
    }

    // 结束设备路径
    data.extend_from_slice(&end.to_bytes());

    data
}

/// 创建硬盘设备路径
pub fn create_hard_drive_device_path(partition: u32, start: u64, size: u64) -> alloc::vec::Vec<u8> {
    let hdd = HardDriveDevicePath::new(partition, start, size);
    let end = EndDevicePath::new();

    let mut data = alloc::vec::Vec::new();

    unsafe {
        let hdd_bytes = core::slice::from_raw_parts(
            &hdd as *const HardDriveDevicePath as *const u8,
            core::mem::size_of::<HardDriveDevicePath>(),
        );
        data.extend_from_slice(hdd_bytes);
    }

    data.extend_from_slice(&end.to_bytes());

    data
}

/// Append a media file-path node to an existing complete device path.
///
/// `base` must end with an End Entire node. The returned path removes that
/// terminal node, appends a normalized UEFI file path, then adds a new End
/// Entire node.
pub fn append_file_path_device_path(base: &[u8], path: &str) -> Option<alloc::vec::Vec<u8>> {
    let end = EndDevicePath::new().to_bytes();
    if base.len() < end.len() || base[base.len() - end.len()..] != end {
        return None;
    }

    let path = normalize_uefi_file_path(path);
    let file_path = FilePathDevicePath::new(&path);
    let mut data = alloc::vec::Vec::with_capacity(base.len() + file_path.len());

    data.extend_from_slice(&base[..base.len() - end.len()]);
    data.extend_from_slice(&file_path);
    data.extend_from_slice(&end);

    Some(data)
}

/// GUID 格式化
pub fn format_guid(guid: &[u8; 16]) -> alloc::string::String {
    alloc::format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid[3], guid[2], guid[1], guid[0],
        guid[5], guid[4],
        guid[7], guid[6],
        guid[8], guid[9],
        guid[10], guid[11], guid[12], guid[13], guid[14], guid[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};

    fn fill_from_lba(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        for byte in buf {
            *byte = lba as u8;
        }
        Ok(())
    }

    fn make_protocol() -> VirtualBlockIoProtocol {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 10, 1024, 512);
        let mut vbio = VirtualBlockIo::new(config);
        vbio.set_physical_read(fill_from_lba);
        VirtualBlockIoProtocol::new(vbio)
    }

    #[test]
    fn read_blocks_handler_delegates_to_virtual_block_io() {
        let mut protocol = make_protocol();
        let ptr = protocol.as_ptr();
        let media_id = protocol.media.media_id;
        let mut buf = [0u8; 1024];

        let status = unsafe {
            ((*ptr).read_blocks)(ptr, media_id, 0, buf.len() as u64, buf.as_mut_ptr().cast())
        };

        assert_eq!(status, UefiStatus::Success as u64);
        assert!(buf[..512].iter().all(|byte| *byte == 10));
        assert!(buf[512..].iter().all(|byte| *byte == 11));
    }

    #[test]
    fn read_blocks_handler_reports_media_change_and_bad_buffer_size() {
        let mut protocol = make_protocol();
        let ptr = protocol.as_ptr();
        let mut buf = [0u8; 512];

        let wrong_media = unsafe {
            ((*ptr).read_blocks)(
                ptr,
                0xDEAD_BEEF,
                0,
                buf.len() as u64,
                buf.as_mut_ptr().cast(),
            )
        };
        assert_eq!(wrong_media, UefiStatus::MediaChanged as u64);

        let bad_size = unsafe {
            ((*ptr).read_blocks)(ptr, protocol.media.media_id, 0, 7, buf.as_mut_ptr().cast())
        };
        assert_eq!(bad_size, UefiStatus::BadBufferSize as u64);
    }

    #[test]
    fn write_blocks_handler_stays_write_protected() {
        let mut protocol = make_protocol();
        let ptr = protocol.as_ptr();
        let buf = [0u8; 512];

        let status = unsafe {
            ((*ptr).write_blocks)(
                ptr,
                protocol.media.media_id,
                0,
                buf.len() as u64,
                buf.as_ptr().cast(),
            )
        };

        assert_eq!(status, UefiStatus::WriteProtected as u64);
    }

    #[test]
    fn read_disk_handler_supports_unaligned_byte_reads() {
        let mut protocol = make_protocol();
        let ptr = protocol.disk_io_ptr();
        let media_id = protocol.media.media_id;
        let mut buf = [0u8; 2];

        let status =
            unsafe { ((*ptr).read_disk)(ptr, media_id, 511, buf.len(), buf.as_mut_ptr().cast()) };

        assert_eq!(status, UefiStatus::Success as u64);
        assert_eq!(buf, [10, 11]);
    }

    #[test]
    fn write_disk_handler_stays_write_protected() {
        let mut protocol = make_protocol();
        let ptr = protocol.disk_io_ptr();
        let media_id = protocol.media.media_id;
        let buf = [0u8; 3];

        let status =
            unsafe { ((*ptr).write_disk)(ptr, media_id, 7, buf.len(), buf.as_ptr().cast()) };

        assert_eq!(status, UefiStatus::WriteProtected as u64);
    }

    #[test]
    fn cdrom_device_path_has_media_node_and_end_node() {
        let path = create_cdrom_device_path(0, 0, 128);

        assert_eq!(path.len(), core::mem::size_of::<CdRomDevicePath>() + 4);
        assert_eq!(path[0], DevicePathType::MEDIA.bits());
        assert_eq!(path[1], MediaSubtype::CdRom as u8);
        assert_eq!(u16::from_le_bytes([path[2], path[3]]), 24);
        assert_eq!(path[path.len() - 4], DevicePathType::END.bits());
        assert_eq!(path[path.len() - 3], 0xFF);
        assert_eq!(
            u16::from_le_bytes([path[path.len() - 2], path[path.len() - 1]]),
            4
        );
    }

    #[test]
    fn hard_drive_device_path_has_media_node_and_end_node() {
        let path = create_hard_drive_device_path(1, 0, 128);

        assert_eq!(path.len(), core::mem::size_of::<HardDriveDevicePath>() + 4);
        assert_eq!(path[0], DevicePathType::MEDIA.bits());
        assert_eq!(path[1], MediaSubtype::HardDrive as u8);
        assert_eq!(u16::from_le_bytes([path[2], path[3]]), 42);
        assert_eq!(path[path.len() - 4], DevicePathType::END.bits());
        assert_eq!(path[path.len() - 3], 0xFF);
        assert_eq!(
            u16::from_le_bytes([path[path.len() - 2], path[path.len() - 1]]),
            4
        );
    }

    #[test]
    fn append_file_path_device_path_replaces_end_node() {
        let base = create_cdrom_device_path(0, 0, 128);
        let full = append_file_path_device_path(&base, "/EFI/BOOT/BOOTX64.EFI").unwrap();
        let cdrom_len = core::mem::size_of::<CdRomDevicePath>();
        let file_node_len = u16::from_le_bytes([full[cdrom_len + 2], full[cdrom_len + 3]]) as usize;

        assert_eq!(full[cdrom_len], DevicePathType::MEDIA.bits());
        assert_eq!(full[cdrom_len + 1], MediaSubtype::FilePath as u8);
        assert_eq!(
            file_node_len,
            4 + ("\\EFI\\BOOT\\BOOTX64.EFI".len() + 1) * 2
        );
        assert_eq!(full[full.len() - 4], DevicePathType::END.bits());
        assert_eq!(full[full.len() - 3], 0xFF);
        assert_eq!(
            u16::from_le_bytes([full[full.len() - 2], full[full.len() - 1]]),
            4
        );
        assert_eq!(full.len(), cdrom_len + file_node_len + 4);
    }

    #[test]
    fn append_file_path_device_path_adds_leading_backslash() {
        let base = create_hard_drive_device_path(1, 0, 128);
        let full = append_file_path_device_path(&base, "EFI\\BOOT\\BOOTAA64.EFI").unwrap();
        let hdd_len = core::mem::size_of::<HardDriveDevicePath>();

        assert_eq!(full[hdd_len + 4], b'\\');
        assert_eq!(full[hdd_len + 5], 0);
    }
}
