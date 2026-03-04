//! GPT 分区表解析
//!
//! 用于检测设备上的分区布局

use crate::FsError;
use alloc::vec::Vec;
use alloc::string::String;
use byteorder::{LittleEndian, ByteOrder};

/// GPT 头
#[repr(C, packed)]
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
}

/// GPT 分区条目
#[derive(Debug, Clone)]
pub struct GptPartition {
    pub type_guid: [u8; 16],
    pub partition_guid: [u8; 16],
    pub start_lba: u64,
    pub end_lba: u64,
    pub attributes: u64,
    pub name: String,
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

    /// Linux 文件系统
    pub const LINUX_FILESYSTEM: [u8; 16] = [
        0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47,
        0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d, 0xe4
    ];
}

/// 解析 GPT 分区表
pub fn parse_gpt(data: &[u8]) -> Result<Vec<GptPartition>, FsError> {
    // GPT 头位于 LBA 1 (通常是偏移 512)
    if data.len() < 1024 {
        return Err(FsError::ReadError);
    }

    // 检查保护性 MBR
    // MBR 的分区类型 0xEE 表示 GPT
    let mbr_partition_type = data[0x1c2];
    if mbr_partition_type != 0xEE {
        return Err(FsError::InvalidSignature);
    }

    // 解析 GPT 头
    let header: GptHeader = unsafe {
        core::mem::transmute_copy(&data[512..])
    };

    // 验证签名
    if header.signature != GptHeader::SIGNATURE {
        return Err(FsError::InvalidSignature);
    }

    // 读取分区条目
    let mut partitions = Vec::new();
    let entry_lba = header.partition_entry_lba as usize;
    let entry_size = header.partition_entry_size as usize;

    for i in 0..header.num_partition_entries as usize {
        let offset = entry_lba * 512 + i * entry_size;
        if offset + entry_size > data.len() {
            break;
        }

        let entry_data = &data[offset..offset + entry_size];

        // 检查是否为空条目 (全零)
        if entry_data[..16].iter().all(|&b| b == 0) {
            continue;
        }

        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&entry_data[0..16]);

        let mut partition_guid = [0u8; 16];
        partition_guid.copy_from_slice(&entry_data[16..32]);

        let start_lba = LittleEndian::read_u64(&entry_data[32..40]);
        let end_lba = LittleEndian::read_u64(&entry_data[40..48]);
        let attributes = LittleEndian::read_u64(&entry_data[48..56]);

        // 名称是 UTF-16LE
        let name = decode_utf16(&entry_data[56..128]);

        partitions.push(GptPartition {
            type_guid,
            partition_guid,
            start_lba,
            end_lba,
            attributes,
            name,
        });
    }

    Ok(partitions)
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
    partition.type_guid == partition_types::ESP
}
