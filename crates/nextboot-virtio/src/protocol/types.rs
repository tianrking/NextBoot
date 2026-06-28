//! UEFI storage protocol types and status mapping.

use core::ffi::c_void;

use crate::{MediaFlags, VirtIoError, VirtualMediaInfo};

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
    pub read_blocks: extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *mut c_void) -> u64,
    pub write_blocks: extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *const c_void) -> u64,
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
        *mut c_void,
    ) -> u64,
    pub write_blocks_ex: extern "efiapi" fn(
        *mut BlockIo2Protocol,
        u32,
        u64,
        *mut BlockIo2Token,
        usize,
        *const c_void,
    ) -> u64,
    pub flush_blocks_ex: extern "efiapi" fn(*mut BlockIo2Protocol, *mut BlockIo2Token) -> u64,
}

/// Block IO 2 Token (异步操作)
#[repr(C)]
pub struct BlockIo2Token {
    pub event: *mut c_void,
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
    pub read_disk: extern "efiapi" fn(*mut DiskIoProtocol, u32, u64, usize, *mut c_void) -> u64,
    pub write_disk: extern "efiapi" fn(*mut DiskIoProtocol, u32, u64, usize, *const c_void) -> u64,
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
        *mut c_void,
    ) -> u64,
    pub write_disk_ex: extern "efiapi" fn(
        *mut DiskIo2Protocol,
        u32,
        u64,
        *mut DiskIo2Token,
        usize,
        *const c_void,
    ) -> u64,
    pub flush_disk_ex: extern "efiapi" fn(*mut DiskIo2Protocol, *mut DiskIo2Token) -> u64,
}

/// Disk IO 2 Token (异步操作)
#[repr(C)]
pub struct DiskIo2Token {
    pub event: *mut c_void,
    pub transaction_status: u64,
}

impl BlockIoMedia {
    /// 从虚拟媒体信息创建
    pub fn from_virtual_info(info: &VirtualMediaInfo) -> Self {
        Self {
            media_id: info.media_id,
            removable_media: info.flags.contains(MediaFlags::REMOVABLE),
            media_present: info.flags.contains(MediaFlags::MEDIA_PRESENT),
            logical_partition: false,
            read_only: info.flags.contains(MediaFlags::READ_ONLY),
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
