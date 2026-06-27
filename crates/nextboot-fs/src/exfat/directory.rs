use crate::{FileAttributes, FileInfo, FsError};
use alloc::string::String;
use alloc::vec::Vec;

use super::fs::ExFat;
use super::model::EntryType;

impl ExFat {
    /// 路径转簇号
    pub(super) fn path_to_cluster(&self, path: &str) -> Result<u32, FsError> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut cluster = self.root_cluster;

        for part in parts {
            let entries = self.read_directory(cluster)?;
            let mut found = false;

            for entry in entries {
                if entry.name.eq_ignore_ascii_case(part) {
                    if !entry.is_dir {
                        return Err(FsError::NotDirectory);
                    }
                    cluster = entry.start_cluster as u32;
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(FsError::DirectoryNotFound);
            }
        }

        Ok(cluster)
    }

    /// 读取目录内容
    pub(super) fn read_directory(&self, cluster: u32) -> Result<Vec<FileInfo>, FsError> {
        let mut entries = Vec::new();
        let mut current_cluster = cluster;

        loop {
            let cluster_data = self.read_cluster(current_cluster)?;

            let mut offset = 0;
            while offset + 32 <= cluster_data.len() {
                let entry_type = cluster_data[offset];

                // 条目结束
                if entry_type == 0 {
                    break;
                }

                // 跳过已删除的条目
                if entry_type == 0xE5 {
                    offset += 32;
                    continue;
                }

                // 文件条目 (0x85)
                if entry_type == EntryType::File as u8 {
                    let file_info = self.parse_file_entry(&cluster_data[offset..])?;
                    if let Some(info) = file_info {
                        entries.push(info);
                    }
                    // 跳过所有次要条目
                    let secondary_count = cluster_data[offset + 1] as usize;
                    offset += 32 * (1 + secondary_count);
                    continue;
                }

                // 其他条目类型跳过
                offset += 32;
            }

            let next_cluster = self.get_next_cluster(current_cluster)?;
            if self.is_end_of_chain(next_cluster) {
                break;
            }
            current_cluster = next_cluster;
        }

        Ok(entries)
    }

    /// 解析文件条目
    fn parse_file_entry(&self, data: &[u8]) -> Result<Option<FileInfo>, FsError> {
        if data.len() < 64 {
            return Ok(None);
        }

        // 主条目
        let secondary_count = data[1] as usize;
        let attributes = u16::from_le_bytes([data[4], data[5]]);

        let is_dir = attributes & 0x0010 != 0;
        let is_hidden = attributes & 0x0002 != 0;
        let is_system = attributes & 0x0004 != 0;

        // 查找流扩展条目和文件名条目
        let mut first_cluster = 0u32;
        let mut data_length = 0u64;
        let mut name_length = 0usize;
        let mut name = String::new();
        let mut contiguous = false;

        let mut offset = 32;
        for _ in 0..secondary_count {
            if offset + 32 > data.len() {
                break;
            }

            let entry_type = data[offset];

            // 流扩展条目
            if entry_type == EntryType::StreamExt as u8 || entry_type == 0xC0 {
                contiguous = data[offset + 1] & 0x02 != 0;
                name_length = data[offset + 3] as usize;
                first_cluster = u32::from_le_bytes([
                    data[offset + 20],
                    data[offset + 21],
                    data[offset + 22],
                    data[offset + 23],
                ]);
                data_length = u64::from_le_bytes([
                    data[offset + 24],
                    data[offset + 25],
                    data[offset + 26],
                    data[offset + 27],
                    data[offset + 28],
                    data[offset + 29],
                    data[offset + 30],
                    data[offset + 31],
                ]);
            }

            // 文件名条目
            if entry_type == EntryType::Name as u8 || entry_type == 0xC1 {
                // 文件名是 UTF-16LE，从偏移 2 开始，每个名称项最多 15 个字符
                let remaining = name_length.saturating_sub(name.chars().count());
                for i in 0..remaining.min(15) {
                    let char_offset = offset + 2 + i * 2;
                    if char_offset + 2 > data.len() {
                        break;
                    }
                    let c = u16::from_le_bytes([data[char_offset], data[char_offset + 1]]);
                    if let Some(ch) = char::from_u32(c as u32) {
                        if ch == '\0' {
                            break;
                        }
                        name.push(ch);
                    }
                }
            }

            offset += 32;
        }

        // 跳过隐藏和系统文件
        if is_hidden || is_system {
            return Ok(None);
        }

        let mut file_attrs = FileAttributes::empty();
        if is_dir {
            file_attrs |= FileAttributes::DIRECTORY;
        }
        if attributes & 0x0001 != 0 {
            file_attrs |= FileAttributes::READ_ONLY;
        }
        if is_hidden {
            file_attrs |= FileAttributes::HIDDEN;
        }
        if is_system {
            file_attrs |= FileAttributes::SYSTEM;
        }

        Ok(Some(FileInfo {
            name,
            size: data_length,
            is_dir,
            attributes: file_attrs,
            start_cluster: first_cluster as u64,
            contiguous,
        }))
    }
}
