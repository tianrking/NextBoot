use crate::{alloc_buffer, read_full_blocks, FileAttributes, FileInfo, FsError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::fs::{Iso9660, IsoDirectoryRecordLocation};

impl Iso9660 {
    /// 读取目录
    pub(super) fn read_directory(&self, lba: u32, size: u64) -> Result<Vec<FileInfo>, FsError> {
        let mut entries = Vec::new();
        let mut current_lba = lba;
        let total_blocks = ((size + self.block_size as u64 - 1) / self.block_size as u64).max(1);

        // 读取目录数据
        let mut dir_data = alloc_buffer(self.block_size as usize)?;

        for _ in 0..total_blocks {
            read_full_blocks(self.block_io.as_ref(), current_lba as u64, &mut dir_data)?;

            let mut offset = 0;
            while offset < dir_data.len() {
                let len = dir_data[offset] as usize;

                // Zero-length records pad to the next logical block.
                if len == 0 {
                    break;
                }

                if offset + len > dir_data.len() {
                    break;
                }

                // 解析目录记录
                if let Some(info) = self.parse_directory_record(&dir_data[offset..offset + len]) {
                    // 跳过 . 和 ..
                    if info.name != "." && info.name != ".." {
                        entries.push(info);
                    }
                }

                offset += len;
            }

            current_lba += 1;
        }

        Ok(entries)
    }

    pub fn directory_record_location(
        &self,
        path: &str,
    ) -> Result<IsoDirectoryRecordLocation, FsError> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(FsError::InvalidPath);
        }

        let mut current_lba = self.root_lba;
        let mut current_size = self.root_size as u64;

        for (index, part) in parts.iter().enumerate() {
            let Some(record) = self.find_directory_record(current_lba, current_size, part)? else {
                return Err(if index + 1 == parts.len() {
                    FsError::FileNotFound
                } else {
                    FsError::DirectoryNotFound
                });
            };

            if index + 1 == parts.len() {
                return Ok(record);
            }
            if !record.is_dir {
                return Err(FsError::NotDirectory);
            }

            current_lba = record.extent_lba;
            current_size = u64::from(record.data_length);
        }

        Err(FsError::InvalidPath)
    }

    fn find_directory_record(
        &self,
        lba: u32,
        size: u64,
        name: &str,
    ) -> Result<Option<IsoDirectoryRecordLocation>, FsError> {
        let mut current_lba = lba;
        let total_blocks = ((size + self.block_size as u64 - 1) / self.block_size as u64).max(1);
        let mut dir_data = alloc_buffer(self.block_size as usize)?;

        for _ in 0..total_blocks {
            read_full_blocks(self.block_io.as_ref(), current_lba as u64, &mut dir_data)?;

            let mut offset = 0usize;
            while offset < dir_data.len() {
                let len = dir_data[offset] as usize;
                if len == 0 {
                    break;
                }
                if offset + len > dir_data.len() {
                    break;
                }

                let record = &dir_data[offset..offset + len];
                if let Some(info) = self.parse_directory_record(record) {
                    if info.name != "." && info.name != ".." && info.name.eq_ignore_ascii_case(name)
                    {
                        let record_offset = u64::from(current_lba)
                            .checked_mul(u64::from(self.block_size))
                            .and_then(|value| value.checked_add(offset as u64))
                            .ok_or(FsError::Corrupted)?;
                        return Ok(Some(IsoDirectoryRecordLocation {
                            record_offset,
                            extent_lba: info.start_cluster as u32,
                            data_length: info.size as u32,
                            is_dir: info.is_dir,
                        }));
                    }
                }

                offset += len;
            }

            current_lba += 1;
        }

        Ok(None)
    }

    /// 解析目录记录
    fn parse_directory_record(&self, data: &[u8]) -> Option<FileInfo> {
        if data.len() < 33 {
            return None;
        }

        let length = data[0] as usize;
        if length < 33 || length > data.len() {
            return None;
        }

        // 读取 LBA (both-endian)
        let extent_lba_le = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        let _extent_lba_be = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);

        // 读取大小 (both-endian)
        let data_length_le = u32::from_le_bytes([data[10], data[11], data[12], data[13]]);
        let _data_length_be = u32::from_be_bytes([data[14], data[15], data[16], data[17]]);

        // 标志
        let flags = data[25];
        let is_dir = flags & 0x02 != 0;
        let is_hidden = flags & 0x01 != 0;

        // 文件名长度
        let name_length = data[32] as usize;
        if name_length == 0 || 33 + name_length > data.len() {
            return None;
        }

        // 读取文件名
        let name_raw = &data[33..33 + name_length];
        let name = self.parse_filename(name_raw);

        // 跳过隐藏文件
        if is_hidden {
            return None;
        }

        let mut attributes = FileAttributes::empty();
        if is_dir {
            attributes |= FileAttributes::DIRECTORY;
        }
        if is_hidden {
            attributes |= FileAttributes::HIDDEN;
        }

        Some(FileInfo {
            name,
            size: data_length_le as u64,
            is_dir,
            attributes,
            start_cluster: extent_lba_le as u64,
            contiguous: true,
        })
    }

    /// 解析文件名
    fn parse_filename(&self, raw: &[u8]) -> String {
        if raw.is_empty() {
            return String::new();
        }

        // 检查是否为 Rock Ridge 扩展名 (以 ; 开头)
        let mut name = String::new();
        let mut ended = false;

        for &b in raw {
            if ended || b == 0 {
                break;
            }
            // 版本号分隔符
            if b == b';' {
                ended = true;
                continue;
            }
            if b >= 0x20 && b < 0x7F {
                name.push(b as char);
            }
        }

        // 移除末尾的点 (如果有的话)
        let name = name.trim_end_matches('.').to_string();

        // 转换为小写
        name.to_lowercase()
    }

    /// 路径转 LBA
    fn path_to_lba(&self, path: &str) -> Result<u32, FsError> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current_lba = self.root_lba;
        let mut current_size = self.root_size as u64;

        for part in parts {
            let entries = self.read_directory(current_lba, current_size)?;
            let mut found = false;

            for entry in entries {
                if entry.name.eq_ignore_ascii_case(part) {
                    if !entry.is_dir {
                        return Err(FsError::NotDirectory);
                    }
                    current_lba = entry.start_cluster as u32;
                    current_size = entry.size;
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(FsError::DirectoryNotFound);
            }
        }

        Ok(current_lba)
    }
}
