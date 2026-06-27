use bitflags::bitflags;

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

/// 硬件设备路径子类型
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum HardwareSubtype {
    /// 供应商特定硬件节点
    Vendor = 0x04,
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

pub(super) const NEXTBOOT_VIRTUAL_DISK_GUID: [u8; 16] = [
    0xf2, 0x5a, 0x77, 0xc1, 0x11, 0x42, 0x55, 0x4f, 0x9f, 0x6f, 0x2c, 0xc5, 0xef, 0x56, 0x67, 0xf0,
];

/// 供应商硬件设备路径。
#[repr(C, packed)]
pub struct VendorHardwareDevicePath {
    pub header: DevicePathHeader,
    pub guid: [u8; 16],
}

impl VendorHardwareDevicePath {
    pub fn new(guid: [u8; 16]) -> Self {
        Self {
            header: DevicePathHeader::new(
                DevicePathType::HARDWARE,
                HardwareSubtype::Vendor as u8,
                20,
            ),
            guid,
        }
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

/// 创建虚拟硬盘控制器设备路径。
///
/// The virtual disk controller must not identify itself as a hard-drive
/// partition. Firmware partition drivers append their own Hard Drive media
/// nodes to this controller path after parsing MBR/GPT.
pub fn create_virtual_disk_controller_device_path() -> alloc::vec::Vec<u8> {
    let vendor = VendorHardwareDevicePath::new(NEXTBOOT_VIRTUAL_DISK_GUID);
    let end = EndDevicePath::new();
    let mut data = alloc::vec::Vec::new();

    unsafe {
        let vendor_bytes = core::slice::from_raw_parts(
            &vendor as *const VendorHardwareDevicePath as *const u8,
            core::mem::size_of::<VendorHardwareDevicePath>(),
        );
        data.extend_from_slice(vendor_bytes);
    }

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
