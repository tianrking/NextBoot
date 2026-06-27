use crate::FsError;

/// exFAT 引导扇区
#[repr(C, packed)]
pub(super) struct ExFatBootSector {
    pub(super) jump: [u8; 3],
    pub(super) fs_name: [u8; 8],
    pub(super) reserved1: [u8; 53],
    pub(super) partition_offset: u64,
    pub(super) volume_length: u64,
    pub(super) fat_offset: u32,
    pub(super) fat_length: u32,
    pub(super) cluster_heap_offset: u32,
    pub(super) cluster_count: u32,
    pub(super) root_cluster: u32,
    pub(super) volume_serial: u32,
    pub(super) fs_revision: u16,
    pub(super) volume_flags: u16,
    pub(super) bytes_per_sector_shift: u8,
    pub(super) sectors_per_cluster_shift: u8,
    pub(super) num_fats: u8,
    pub(super) drive_select: u8,
    pub(super) percent_in_use: u8,
    pub(super) reserved2: [u8; 7],
    pub(super) boot_code: [u8; 390],
    pub(super) signature: u16,
}

/// exFAT 文件目录条目 (主条目)
#[repr(C, packed)]
struct FileEntry {
    pub(super) entry_type: u8,
    pub(super) secondary_count: u8,
    pub(super) checksum: u16,
    pub(super) attributes: u16,
    pub(super) reserved1: u16,
    pub(super) create_time: u32,
    pub(super) create_time_ms: u8,
    pub(super) modify_time: u32,
    pub(super) modify_time_ms: u8,
    pub(super) access_time: u32,
    pub(super) access_time_ms: u8,
    pub(super) create_10ms: u8,
    pub(super) modify_10ms: u8,
    pub(super) access_10ms: u8,
    pub(super) reserved2: [u8; 8],
}

/// exFAT 流扩展条目
#[repr(C, packed)]
struct StreamExtEntry {
    pub(super) entry_type: u8,
    pub(super) general_secondary_flags: u8,
    pub(super) reserved1: u8,
    pub(super) name_length: u8,
    pub(super) name_hash: u16,
    pub(super) reserved2: u16,
    pub(super) valid_data_length: u64,
    pub(super) reserved3: u32,
    pub(super) first_cluster: u32,
    pub(super) data_length: u64,
}

/// exFAT 文件名条目
#[repr(C, packed)]
struct NameEntry {
    pub(super) entry_type: u8,
    pub(super) general_secondary_flags: u8,
    pub(super) reserved1: u8,
    pub(super) name_length: u8,
    pub(super) name_hash: u16,
    pub(super) reserved2: u16,
    // 文件名数据紧跟其后
}

/// 条目类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum EntryType {
    File = 0x85,
    StreamExt = 0xC0,
    Name = 0xC1,
    VendorExt = 0xA0,
    VendorAlloc = 0xA1,
    Bitmap = 0x81,
    Upcase = 0x82,
    VolumeLabel = 0x83,
}

impl TryFrom<u8> for EntryType {
    type Error = FsError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x85 => Ok(EntryType::File),
            0xC0 => Ok(EntryType::StreamExt),
            0xC1 => Ok(EntryType::Name),
            0x81 => Ok(EntryType::Bitmap),
            0x82 => Ok(EntryType::Upcase),
            0x83 => Ok(EntryType::VolumeLabel),
            _ => Err(FsError::InvalidSignature),
        }
    }
}
