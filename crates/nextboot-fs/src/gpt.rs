//! GPT 分区表解析
//!
//! 用于检测设备上的分区布局

use crate::FsError;
use alloc::vec::Vec;
use alloc::string::String;
use byteorder::{LittleEndian, ByteOrder};
use alloc::format;

/// GPT 头
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GptHeader {
    pub signature: [u8; 8],
    pub revision: u32,
    pub header_size: u32,
    pub header_crc: u32,
    pub reserved: u32,
    pub my_lba: u64,
    pub alternate_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: [u8; 16],
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub partition_entry_size: u32,
    pub partition_array_crc: u32,
}

impl GptHeader {
    pub const SIGNATURE: [u8; 8] = *b"EFI PART";

    /// 验证 GPT 头
    pub fn is_valid(&self) -> bool {
        self.signature == Self::SIGNATURE
            && self.revision >= 0x00010000
            && self.header_size >= 92
    }

    /// 获取磁盘 GUID 字符串
    pub fn disk_guid_string(&self) -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.disk_guid[3], self.disk_guid[2], self.disk_guid[1], self.disk_guid[0],
            self.disk_guid[5], self.disk_guid[4],
            self.disk_guid[7], self.disk_guid[6],
            self.disk_guid[8], self.disk_guid[9],
            self.disk_guid[10], self.disk_guid[11], self.disk_guid[12],
            self.disk_guid[13], self.disk_guid[14], self.disk_guid[15]
        )
    }
}

/// GPT 分区条目
#[derive(Debug, Clone)]
pub struct GptPartition {
    /// 分区类型 GUID
    pub type_guid: [u8; 16],
    /// 分区 GUID
    pub partition_guid: [u8; 16],
    /// 起始 LBA
    pub start_lba: u64,
    /// 结束 LBA (包含)
    pub end_lba: u64,
    /// 属性标志
    pub attributes: u64,
    /// 分区名称
    pub name: String,
}

impl GptPartition {
    /// 获取分区大小 (字节)
    pub fn size_bytes(&self, block_size: u32) -> u64 {
        (self.end_lba - self.start_lba + 1) * block_size as u64
    }

    /// 获取分区大小 (块数)
    pub fn size_blocks(&self) -> u64 {
        self.end_lba - self.start_lba + 1
    }

    /// 检查是否为 ESP 分区
    pub fn is_esp(&self) -> bool {
        self.type_guid == partition_types::ESP
    }

    /// 检查是否为 Microsoft 基本数据分区
    pub fn is_microsoft_basic(&self) -> bool {
        self.type_guid == partition_types::MICROSOFT_BASIC
    }

    /// 检查是否为 Linux 分区
    pub fn is_linux(&self) -> bool {
        self.type_guid == partition_types::LINUX_FILESYSTEM
            || self.type_guid == partition_types::LINUX_LVM
            || self.type_guid == partition_types::LINUX_RAID
    }

    /// 获取类型描述
    pub fn type_description(&self) -> &'static str {
        if self.is_esp() {
            return "EFI System Partition";
        }
        if self.is_microsoft_basic() {
            return "Microsoft Basic Data";
        }
        if self.type_guid == partition_types::MICROSOFT_RESERVED {
            return "Microsoft Reserved";
        }
        if self.type_guid == partition_types::LINUX_FILESYSTEM {
            return "Linux Filesystem";
        }
        if self.type_guid == partition_types::LINUX_SWAP {
            return "Linux Swap";
        }
        if self.type_guid == partition_types::LINUX_LVM {
            return "Linux LVM";
        }
        if self.type_guid == partition_types::APPLE_HFS {
            return "Apple HFS+";
        }
        "Unknown"
    }
}

/// 已知的分区类型 GUID
pub mod partition_types {
    /// EFI 系统分区
    pub const ESP: [u8; 16] = [
        0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11,
        0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b
    ];

    /// Microsoft 基本数据分区
    pub const MICROSOFT_BASIC: [u8; 16] = [
        0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44,
        0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7
    ];

    /// Microsoft 保留分区
    pub const MICROSOFT_RESERVED: [u8; 16] = [
        0x16, 0xe3, 0xc9, 0xe3, 0x5c, 0x0b, 0xb9, 0x45,
        0x9c, 0xfc, 0xa1, 0x02, 0x14, 0x94, 0x96, 0x13
    ];

    /// Linux 文件系统
    pub const LINUX_FILESYSTEM: [u8; 16] = [
        0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47,
        0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d, 0xe4
    ];

    /// Linux Swap
    pub const LINUX_SWAP: [u8; 16] = [
        0x06, 0x57, 0xfd, 0x6d, 0xa4, 0xab, 0x43, 0xc4,
        0x84, 0xe5, 0x09, 0x33, 0xc8, 0x4b, 0x4f, 0x4f
    ];

    /// Linux LVM
    pub const LINUX_LVM: [u8; 16] = [
        0xe6, 0xd6, 0xd3, 0x79, 0xf5, 0x87, 0x6b, 0x44,
        0xa7, 0x23, 0x95, 0x2c, 0x71, 0x8a, 0x5d, 0x9b
    ];

    /// Linux RAID
    pub const LINUX_RAID: [u8; 16] = [
        0xa1, 0x9d, 0x8b, 0x27, 0x8b, 0xd9, 0x47, 0x46,
        0xa9, 0x8f, 0x4e, 0x09, 0x09, 0xf9, 0xc4, 0x6d
    ];

    /// Apple HFS+
    pub const APPLE_HFS: [u8; 16] = [
        0x00, 0x53, 0x46, 0x48, 0x00, 0x00, 0xaa, 0x11,
        0xaa, 0x11, 0x00, 0x30, 0x65, 0x43, 0xec, 0xac
    ];

    /// BIOS 引导分区
    pub const BIOS_BOOT: [u8; 16] = [
        0x21, 0x68, 0x61, 0x48, 0x64, 0x49, 0x6e, 0x6f,
        0x74, 0x4e, 0x61, 0x6d, 0x65, 0x53, 0x70, 0x65
    ];
}

/// GPT 分区表
#[derive(Debug, Clone)]
pub struct GptDisk {
    /// GPT 头
    pub header: GptHeader,
    /// 分区列表
    pub partitions: Vec<GptPartition>,
    /// 块大小
    pub block_size: u32,
}

impl GptDisk {
    /// 从原始数据解析 GPT
    pub fn parse(data: &[u8], block_size: u32) -> Result<Self, FsError> {
        // 检查保护性 MBR
        if !check_protective_mbr(data) {
            return Err(FsError::InvalidSignature);
        }

        // 解析 GPT 头 (LBA 1)
        let header_offset = block_size as usize;
        if data.len() < header_offset + core::mem::size_of::<GptHeader>() {
            return Err(FsError::ReadError);
        }

        let header: GptHeader = unsafe {
            core::ptr::read_unaligned(data[header_offset..].as_ptr() as *const GptHeader)
        };

        if !header.is_valid() {
            return Err(FsError::InvalidSignature);
        }

        // 读取分区条目
        let partitions = parse_partition_entries(data, &header, block_size)?;

        Ok(Self {
            header,
            partitions,
            block_size,
        })
    }

    /// 获取 ESP 分区
    pub fn get_esp_partition(&self) -> Option<&GptPartition> {
        self.partitions.iter().find(|p| p.is_esp())
    }

    /// 获取数据分区 (第一个非 ESP 的可读分区)
    pub fn get_data_partition(&self) -> Option<&GptPartition> {
        self.partitions.iter()
            .filter(|p| !p.is_esp())
            .filter(|p| p.is_microsoft_basic() || p.is_linux())
            .next()
    }

    /// 查找分区
    pub fn find_partition(&self, name: &str) -> Option<&GptPartition> {
        self.partitions.iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// 检查保护性 MBR
fn check_protective_mbr(data: &[u8]) -> bool {
    if data.len() < 512 {
        return false;
    }

    // 检查 MBR 签名
    if data[510] != 0x55 || data[511] != 0xAA {
        return false;
    }

    // 检查第一个分区条目是否为 GPT 保护类型 (0xEE)
    let partition_type = data[0x1C2];
    partition_type == 0xEE
}

/// 解析分区条目
fn parse_partition_entries(
    data: &[u8],
    header: &GptHeader,
    block_size: u32,
) -> Result<Vec<GptPartition>, FsError> {
    let mut partitions = Vec::new();

    let entry_lba = header.partition_entry_lba;
    let entry_offset = entry_lba as usize * block_size as usize;
    let entry_size = header.partition_entry_size as usize;
    let num_entries = header.num_partition_entries as usize;

    for i in 0..num_entries {
        let offset = entry_offset + i * entry_size;
        if offset + entry_size > data.len() {
            break;
        }

        let entry_data = &data[offset..offset + entry_size];

        // 检查是否为空条目 (类型 GUID 全零)
        if entry_data[..16].iter().all(|&b| b == 0) {
            continue;
        }

        let partition = parse_single_partition(entry_data);
        partitions.push(partition);
    }

    Ok(partitions)
}

/// 解析单个分区条目
fn parse_single_partition(data: &[u8]) -> GptPartition {
    let mut type_guid = [0u8; 16];
    type_guid.copy_from_slice(&data[0..16]);

    let mut partition_guid = [0u8; 16];
    partition_guid.copy_from_slice(&data[16..32]);

    let start_lba = LittleEndian::read_u64(&data[32..40]);
    let end_lba = LittleEndian::read_u64(&data[40..48]);
    let attributes = LittleEndian::read_u64(&data[48..56]);

    // 名称是 UTF-16LE
    let name = decode_utf16(&data[56..128]);

    GptPartition {
        type_guid,
        partition_guid,
        start_lba,
        end_lba,
        attributes,
        name,
    }
}

/// 解码 UTF-16LE 字符串
fn decode_utf16(data: &[u8]) -> String {
    let mut s = String::new();
    for chunk in data.chunks(2) {
        if chunk.len() < 2 {
            break;
        }
        let c = u16::from_le_bytes([chunk[0], chunk[1]]);
        if c == 0 {
            break;
        }
        if let Some(ch) = char::from_u32(c as u32) {
            s.push(ch);
        }
    }
    s
}

/// 检查分区是否为 ESP
pub fn is_esp_partition(partition: &GptPartition) -> bool {
    partition.is_esp()
}

/// 检测块大小 (512B 或 4K)
pub fn detect_block_size(data: &[u8]) -> u32 {
    // 尝试 512 字节
    if data.len() >= 1024 {
        let gpt_offset_512 = 512;
        if data.len() > gpt_offset_512 + 8 {
            if &data[gpt_offset_512..gpt_offset_512 + 8] == b"EFI PART" {
                return 512;
            }
        }
    }

    // 尝试 4096 字节
    if data.len() >= 8192 {
        let gpt_offset_4k = 4096;
        if data.len() > gpt_offset_4k + 8 {
            if &data[gpt_offset_4k..gpt_offset_4k + 8] == b"EFI PART" {
                return 4096;
            }
        }
    }

    // 默认 512
    512
}

/// MBR 分区表 (用于兼容性检测)
#[derive(Debug, Clone)]
pub struct MbrPartition {
    pub bootable: bool,
    pub partition_type: u8,
    pub start_lba: u32,
    pub total_sectors: u32,
}

/// 解析 MBR 分区表
pub fn parse_mbr(data: &[u8]) -> Option<Vec<MbrPartition>> {
    if data.len() < 512 {
        return None;
    }

    // 检查 MBR 签名
    if data[510] != 0x55 || data[511] != 0xAA {
        return None;
    }

    let mut partitions = Vec::new();

    // 4 个主分区
    for i in 0..4 {
        let offset = 0x1BE + i * 16;

        let bootable = data[offset] == 0x80;
        let partition_type = data[offset + 4];
        let start_lba = u32::from_le_bytes([
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
        ]);
        let total_sectors = u32::from_le_bytes([
            data[offset + 12],
            data[offset + 13],
            data[offset + 14],
            data[offset + 15],
        ]);

        if partition_type != 0 {
            partitions.push(MbrPartition {
                bootable,
                partition_type,
                start_lba,
                total_sectors,
            });
        }
    }

    if partitions.is_empty() {
        None
    } else {
        Some(partitions)
    }
}
