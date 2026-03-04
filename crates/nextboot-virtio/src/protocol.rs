//! UEFI Block IO Protocol 实现
//!
//! 定义与 UEFI 兼容的协议接口

use bitflags::bitflags;

/// UEFI Block IO Protocol GUID
pub const BLOCK_IO_GUID: [u8; 16] = [
    0x96, 0x5e, 0x3b, 0x09, 0x63, 0x30, 0xd3, 0x11,
    0x8d, 0xbd, 0x00, 0xa0, 0xc9, 0x06, 0xec, 0x9b,
];

/// UEFI Block IO 2 Protocol GUID
pub const BLOCK_IO_2_GUID: [u8; 16] = [
    0xa8, 0x63, 0x9a, 0x14, 0x5d, 0x37, 0x7b, 0x4e,
    0xa9, 0x88, 0x6c, 0x42, 0xf4, 0x3e, 0x4c, 0x9a,
];

/// 设备路径协议 GUID
pub const DEVICE_PATH_GUID: [u8; 16] = [
    0x9b, 0x9a, 0x2d, 0x09, 0x62, 0x30, 0xd3, 0x11,
    0x8d, 0xbd, 0x00, 0xa0, 0xc9, 0x06, 0xec, 0x9b,
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
}

/// 设备路径节点头
#[repr(C, packed)]
pub struct DevicePathHeader {
    pub type_: u8,
    pub subtype: u8,
    pub length: [u8; 2],  // Little-endian u16
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
            header: DevicePathHeader {
                type_: DevicePathType::MEDIA.bits(),
                subtype: MediaSubtype::CdRom as u8,
                length: 24u16.to_le_bytes(),
            },
            boot_entry,
            partition_start: start,
            partition_size: size,
        }
    }
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
            header: DevicePathHeader {
                type_: DevicePathType::END.bits(),
                subtype: 0xFF,  // End Entire
                length: 4u16.to_le_bytes(),
            },
        }
    }
}

/// 向 UEFI 注册虚拟设备
///
/// # 安全性
/// 此函数直接与 UEFI 固件交互，需要确保所有参数正确
pub unsafe fn register_virtual_device(
    _handle: *mut core::ffi::c_void,
    _device_path: *const u8,
    _block_io: *const BlockIoProtocol,
) -> i64 {
    // TODO: 调用 UEFI BootServices.InstallMultipleProtocolInterfaces
    // 这需要访问 SystemTable，实际实现时需要传入
    0
}

/// Block IO Protocol 结构体 (UEFI 定义)
#[repr(C)]
pub struct BlockIoProtocol {
    pub revision: u64,
    pub media: *const BlockIoMedia,
    pub reset: extern "efiapi" fn(*mut BlockIoProtocol, bool) -> i64,
    pub read_blocks: extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *mut core::ffi::c_void) -> i64,
    pub write_blocks: extern "efiapi" fn(*mut BlockIoProtocol, u32, u64, u64, *const core::ffi::c_void) -> i64,
    pub flush_blocks: extern "efiapi" fn(*mut BlockIoProtocol) -> i64,
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
