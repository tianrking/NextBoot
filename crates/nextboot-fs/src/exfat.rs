//! exFAT 文件系统实现
//!
//! 用于 Data 分区，支持 >4GB 文件

use crate::{FileSystem, FileSystemType, FileInfo, FsError, BlockIoOps, FileAttributes, alloc_buffer};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use byteorder::{LittleEndian, ByteOrder};

/// exFAT 文件系统
pub struct ExFat {
    /// 扇区大小 (字节)
    sector_size: u32,
    /// 簇大小 (字节)
    cluster_size: u64,
    /// 总簇数
    total_clusters: u32,
    /// 根目录簇号
    root_cluster: u32,
    /// FAT 起始扇区
    fat_offset: u64,
    /// 簇堆起始扇区
    cluster_heap_offset: u64,
    /// 分区偏移 (字节)
    partition_offset: u64,
    /// 卷序列号
    volume_serial: u32,
    /// FAT 缓存
    fat_cache: BTreeMap<u32, u32>,
}

/// exFAT 引导扇区
#[repr(C, packed)]
struct ExFatBootSector {
    jump: [u8; 3],
    fs_name: [u8; 8],
    reserved1: [u8; 53],
    partition_offset: u64,
    volume_length: u64,
    fat_offset: u32,
    fat_length: u32,
    cluster_heap_offset: u32,
    cluster_count: u32,
    root_cluster: u32,
    volume_serial: u32,
    fs_revision: u16,
    volume_flags: u16,
    bytes_per_sector_shift: u8,
    sectors_per_cluster_shift: u8,
    num_fats: u8,
    drive_select: u8,
    percent_in_use: u8,
    reserved2: [u8; 7],
    boot_code: [u8; 390],
    signature: u16,
}

/// exFAT 文件目录条目 (主条目)
#[repr(C, packed)]
struct FileEntry {
    entry_type: u8,
    secondary_count: u8,
    checksum: u16,
    attributes: u16,
    reserved1: u16,
    create_time: u32,
    create_time_ms: u8,
    modify_time: u32,
    modify_time_ms: u8,
    access_time: u32,
    access_time_ms: u8,
    create_10ms: u8,
    modify_10ms: u8,
    access_10ms: u8,
    reserved2: [u8; 8],
}

/// exFAT 流扩展条目
#[repr(C, packed)]
struct StreamExtEntry {
    entry_type: u8,
    general_secondary_flags: u8,
    reserved1: u8,
    name_length: u8,
    name_hash: u16,
    reserved2: u16,
    valid_data_length: u64,
    reserved3: u32,
    first_cluster: u32,
    data_length: u64,
}

/// exFAT 文件名条目
#[repr(C, packed)]
struct NameEntry {
    entry_type: u8,
    general_secondary_flags: u8,
    reserved1: u8,
    name_length: u8,
    name_hash: u16,
    reserved2: u16,
    // 文件名数据紧跟其后
}

/// 条目类型
#[derive(Debug, Clone, Copy, PartialEq)]
enum EntryType {
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

impl FileSystem for ExFat {
    const FS_TYPE: FileSystemType = FileSystemType::ExFat;

    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError> {
        let mut boot_buf = alloc_buffer(block_io.block_size() as usize)?;
        block_io.read_blocks(0, &mut boot_buf)?;

        let boot: ExFatBootSector = unsafe {
            core::mem::transmute_copy(&boot_buf[..core::mem::size_of::<ExFatBootSector>()])
        };

        // 验证 exFAT 签名
        if &boot.fs_name != b"EXFAT   " {
            return Err(FsError::InvalidSignature);
        }

        // 检查引导签名
        if boot.signature != 0xAA55 {
            return Err(FsError::InvalidSignature);
        }

        let sector_size = 1u32 << boot.bytes_per_sector_shift;
        let cluster_size = (sector_size as u64) << boot.sectors_per_cluster_shift;

        Ok(Self {
            sector_size,
            cluster_size,
            total_clusters: boot.cluster_count,
            root_cluster: boot.root_cluster,
            fat_offset: boot.fat_offset as u64,
            cluster_heap_offset: boot.cluster_heap_offset as u64,
            partition_offset: boot.partition_offset,
            volume_serial: boot.volume_serial,
            fat_cache: BTreeMap::new(),
        })
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        let cluster = if path == "/" || path.is_empty() {
            self.root_cluster
        } else {
            self.path_to_cluster(path)?
        };

        self.read_directory(cluster)
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let info = self.stat(path)?;
        if info.is_dir {
            return Err(FsError::NotFile);
        }

        let file_size = info.size;
        if offset >= file_size {
            return Ok(0);
        }

        let remaining = file_size - offset;
        let to_read = buf.len().min(remaining as usize);

        let mut cluster = info.start_cluster as u32;
        let cluster_size = self.cluster_size;

        // 跳过偏移的簇
        let skip_clusters = offset / cluster_size;
        for _ in 0..skip_clusters {
            cluster = self.get_next_cluster(cluster)?;
        }

        // 读取数据
        let mut bytes_read = 0;
        let mut in_cluster_offset = (offset % cluster_size) as usize;

        while bytes_read < to_read && !self.is_end_of_chain(cluster) {
            let cluster_data = self.read_cluster(cluster)?;

            let available = cluster_data.len() - in_cluster_offset;
            let needed = to_read - bytes_read;
            let copy_size = available.min(needed);

            buf[bytes_read..bytes_read + copy_size].copy_from_slice(
                &cluster_data[in_cluster_offset..in_cluster_offset + copy_size]
            );

            bytes_read += copy_size;
            in_cluster_offset = 0;

            if bytes_read < to_read {
                cluster = self.get_next_cluster(cluster)?;
            }
        }

        Ok(bytes_read)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        if path == "/" || path.is_empty() {
            return Ok(FileInfo::new(String::from("/"), 0, true, self.root_cluster as u64));
        }

        let (dir, name) = crate::split_path(path);
        let entries = self.read_dir(&dir)?;

        for entry in entries {
            if entry.name.eq_ignore_ascii_case(&name) {
                return Ok(entry);
            }
        }

        Err(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.sector_size
    }
}

impl ExFat {
    /// 簇号转扇区号
    fn cluster_to_sector(&self, cluster: u32) -> u64 {
        self.cluster_heap_offset as u64 + ((cluster - 2) as u64) * (self.cluster_size / self.sector_size as u64)
    }

    /// 读取簇数据
    fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>, FsError> {
        if cluster < 2 || cluster >= self.total_clusters + 2 {
            return Err(FsError::InvalidArgument);
        }

        let sectors_per_cluster = (self.cluster_size / self.sector_size as u64) as usize;
        let mut buf = alloc_buffer(self.cluster_size as usize)?;
        // 简化实现：需要 block_io 引用
        Ok(buf)
    }

    /// 获取下一个簇号
    fn get_next_cluster(&self, cluster: u32) -> Result<u32, FsError> {
        if let Some(&next) = self.fat_cache.get(&cluster) {
            return Ok(next);
        }

        // exFAT FAT 条目是 32 位
        let fat_offset = self.fat_offset * self.sector_size as u64;
        let entry_offset = fat_offset + (cluster as u64) * 4;

        // 简化实现：返回 EOF
        Ok(0xFFFFFFFF)
    }

    /// 检查是否为链结束
    fn is_end_of_chain(&self, cluster: u32) -> bool {
        cluster >= 0xFFFFFFF8
    }

    /// 路径转簇号
    fn path_to_cluster(&self, path: &str) -> Result<u32, FsError> {
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
    fn read_directory(&self, cluster: u32) -> Result<Vec<FileInfo>, FsError> {
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

            if self.is_end_of_chain(current_cluster) {
                break;
            }
            current_cluster = self.get_next_cluster(current_cluster)?;
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
        let mut name = String::new();

        let mut offset = 32;
        for _ in 0..secondary_count {
            if offset + 32 > data.len() {
                break;
            }

            let entry_type = data[offset];

            // 流扩展条目
            if entry_type == EntryType::StreamExt as u8 || entry_type == 0xC0 {
                first_cluster = u32::from_le_bytes([
                    data[offset + 24],
                    data[offset + 25],
                    data[offset + 26],
                    data[offset + 27],
                ]);
                data_length = u64::from_le_bytes([
                    data[offset + 32],
                    data[offset + 33],
                    data[offset + 34],
                    data[offset + 35],
                    data[offset + 36],
                    data[offset + 37],
                    data[offset + 38],
                    data[offset + 39],
                ]);
            }

            // 文件名条目
            if entry_type == EntryType::Name as u8 || entry_type == 0xC1 {
                let name_length = data[offset + 3] as usize;
                // 文件名是 UTF-16LE，从偏移 4 开始
                for i in 0..name_length.min(15) {
                    let char_offset = offset + 4 + i * 2;
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
        }))
    }
}

/// 检查是否为有效的 exFAT 文件系统
pub fn is_exfat(data: &[u8]) -> bool {
    if data.len() < 11 {
        return false;
    }

    // exFAT 跳转指令和签名
    data[0] == 0xEB
        && data[1] == 0x76
        && data[2] == 0x90
        && &data[3..11] == b"EXFAT   "
}

/// exFAT 文件系统信息
#[derive(Debug, Clone)]
pub struct ExFatInfo {
    pub volume_serial: u32,
    pub total_size: u64,
    pub cluster_size: u64,
    pub total_clusters: u32,
}
