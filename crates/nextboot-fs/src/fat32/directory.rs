use crate::{FileAttributes, FileInfo, FsError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::fs::Fat32;

impl Fat32 {
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
        let mut lfn_buffer = String::new();

        loop {
            let cluster_data = self.read_cluster(current_cluster)?;

            // 解析目录条目
            for chunk in cluster_data.chunks(32) {
                if chunk.is_empty() || chunk[0] == 0 {
                    break;
                }

                // 跳过删除的条目
                if chunk[0] == 0xE5 {
                    lfn_buffer.clear();
                    continue;
                }

                let attr = chunk[11];

                // 长文件名条目
                if attr == 0x0F {
                    self.parse_lfn_entry(chunk, &mut lfn_buffer);
                    continue;
                }

                // 跳过卷标
                if attr & 0x08 != 0 {
                    lfn_buffer.clear();
                    continue;
                }

                // 解析标准目录条目
                let name = if lfn_buffer.is_empty() {
                    self.parse_short_name(&chunk[0..11])
                } else {
                    let name = lfn_buffer.clone();
                    lfn_buffer.clear();
                    name
                };

                let cluster_high = u16::from_le_bytes([chunk[20], chunk[21]]) as u32;
                let cluster_low = u16::from_le_bytes([chunk[26], chunk[27]]) as u32;
                let file_cluster = (cluster_high << 16) | cluster_low;
                let file_size = u32::from_le_bytes([chunk[28], chunk[29], chunk[30], chunk[31]]);

                let is_dir = attr & 0x10 != 0;

                // 跳过 . 和 ..
                if name == "." || name == ".." {
                    continue;
                }

                let attributes = FileAttributes::from_bits_truncate(attr);

                entries.push(FileInfo {
                    name,
                    size: file_size as u64,
                    is_dir,
                    attributes,
                    start_cluster: file_cluster as u64,
                    contiguous: false,
                });
            }

            let next_cluster = self.get_next_cluster(current_cluster)?;
            if self.is_end_of_chain(next_cluster) {
                break;
            }
            current_cluster = next_cluster;
        }

        Ok(entries)
    }

    /// 解析短文件名
    fn parse_short_name(&self, raw: &[u8]) -> String {
        let name: String = String::from_utf8_lossy(&raw[0..8]).trim_end().to_string();
        let ext: String = String::from_utf8_lossy(&raw[8..11]).trim_end().to_string();

        if ext.is_empty() {
            name
        } else {
            alloc::format!("{}.{}", name, ext)
        }
    }

    /// 解析长文件名条目
    fn parse_lfn_entry(&self, chunk: &[u8], buffer: &mut String) {
        let is_last = chunk[0] & 0x40 != 0;

        // 读取 UTF-16 字符
        let mut chars = Vec::new();

        // 第一段: 5 个字符 (偏移 1-10)
        for i in 0..5 {
            let offset = 1 + i * 2;
            let c = u16::from_le_bytes([chunk[offset], chunk[offset + 1]]);
            if c != 0 && c != 0xFFFF {
                chars.push(c);
            }
        }

        // 第二段: 6 个字符 (偏移 14-25)
        for i in 0..6 {
            let offset = 14 + i * 2;
            let offset = offset.min(chunk.len() - 2);
            let c = u16::from_le_bytes([chunk[offset], chunk[offset + 1]]);
            if c != 0 && c != 0xFFFF {
                chars.push(c);
            }
        }

        // 第三段: 2 个字符 (偏移 28-31)
        for i in 0..2 {
            let offset = 28 + i * 2;
            if offset + 2 <= chunk.len() {
                let c = u16::from_le_bytes([chunk[offset], chunk[offset + 1]]);
                if c != 0 && c != 0xFFFF {
                    chars.push(c);
                }
            }
        }

        // 转换为字符串
        let name_part: String = chars
            .iter()
            .filter_map(|&c| char::from_u32(c as u32))
            .collect();

        if is_last {
            buffer.clear();
        }
        buffer.insert_str(0, &name_part);
    }
}
