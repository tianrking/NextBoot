//! FAT32 文件系统实现
//!
//! 仅支持读取，用于 ESP 分区和 Data 分区

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// FAT32 文件系统
pub struct Fat32 {
    /// 底层块设备
    block_io: SharedBlockIo,
    /// 块大小
    block_size: u32,
    /// 每簇扇区数
    sectors_per_cluster: u8,
    /// 簇大小 (字节)
    cluster_size: u32,
    /// FAT 表起始扇区
    fat_start: u64,
    /// FAT 表大小 (扇区)
    fat_size: u64,
    /// FAT 表数量
    num_fats: u8,
    /// 数据区起始扇区
    data_start: u64,
    /// 根目录簇号
    root_cluster: u32,
    /// 总簇数
    total_clusters: u32,
    /// 卷标
    volume_label: String,
    /// FAT 缓存 (部分)
    fat_cache: BTreeMap<u32, u32>,
}

/// FAT32 引导扇区
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Fat32BootSector {
    jump: [u8; 3],
    oem: [u8; 8],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    root_entries: u16,
    total_sectors_16: u16,
    media_type: u8,
    sectors_per_fat_16: u16,
    sectors_per_track: u16,
    num_heads: u16,
    hidden_sectors: u32,
    total_sectors_32: u32,
    // FAT32 扩展
    sectors_per_fat_32: u32,
    ext_flags: u16,
    fs_version: u16,
    root_cluster: u32,
    fs_info_sector: u16,
    backup_boot_sector: u16,
    reserved: [u8; 12],
    drive_num: u8,
    reserved1: u8,
    boot_signature: u8,
    volume_id: u32,
    volume_label: [u8; 11],
    fs_type: [u8; 8],
}

/// FAT 目录条目 (32 字节)
#[repr(C, packed)]
struct FatDirEntry {
    name: [u8; 11],
    attr: u8,
    nt_reserved: u8,
    create_time_tenth: u8,
    create_time: u16,
    create_date: u16,
    last_access_date: u16,
    cluster_high: u16,
    modify_time: u16,
    modify_date: u16,
    cluster_low: u16,
    file_size: u32,
}

/// 长文件名条目
struct LfnEntry {
    seq: u8,
    name1: [u16; 5],
    attr: u8,
    type_: u8,
    checksum: u8,
    name2: [u16; 6],
    reserved: u16,
    name3: [u16; 2],
}

impl FileSystem for Fat32 {
    const FS_TYPE: FileSystemType = FileSystemType::Fat32;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let mut boot_buf = alloc_buffer(block_io.block_size() as usize)?;
        read_full_blocks(block_io.as_ref(), 0, &mut boot_buf)?;

        // 安全转换
        let boot_sector: Fat32BootSector =
            unsafe { core::ptr::read_unaligned(boot_buf.as_ptr() as *const Fat32BootSector) };

        // 验证 FAT32
        if boot_sector.bytes_per_sector == 0 {
            return Err(FsError::InvalidSignature);
        }

        let block_size = boot_sector.bytes_per_sector as u32;
        if block_size != block_io.block_size() {
            return Err(FsError::BlockSizeMismatch);
        }

        let sectors_per_cluster = boot_sector.sectors_per_cluster;
        let cluster_size = block_size * sectors_per_cluster as u32;

        // 计算 FAT32 参数
        let fat_start = boot_sector.reserved_sectors as u64;
        let fat_size = boot_sector.sectors_per_fat_32 as u64;
        let num_fats = boot_sector.num_fats;

        // 数据区起始位置
        let data_start = fat_start + fat_size * num_fats as u64;

        // 计算总簇数
        let total_sectors = if boot_sector.total_sectors_32 != 0 {
            boot_sector.total_sectors_32 as u64
        } else {
            boot_sector.total_sectors_16 as u64
        };

        let data_sectors = total_sectors - data_start;
        let total_clusters = (data_sectors / sectors_per_cluster as u64) as u32;

        // 解析卷标
        let volume_label = String::from_utf8_lossy(&boot_buf[71..82])
            .trim_end()
            .to_string();

        Ok(Self {
            block_io,
            block_size,
            sectors_per_cluster,
            cluster_size,
            fat_start,
            fat_size,
            num_fats,
            data_start,
            root_cluster: boot_sector.root_cluster,
            total_clusters,
            volume_label,
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
        let cluster_size = self.cluster_size as u64;

        // 跳过偏移的簇
        let skip_clusters = offset / cluster_size;
        for _ in 0..skip_clusters {
            cluster = self.get_next_cluster(cluster)?;
        }

        // 读取数据
        let mut bytes_read = 0;
        let mut in_cluster_offset = (offset % cluster_size) as usize;

        while bytes_read < to_read && cluster >= 2 && cluster < 0x0FFFFFF8 {
            let cluster_data = self.read_cluster(cluster)?;

            let available = cluster_data.len() - in_cluster_offset;
            let needed = to_read - bytes_read;
            let copy_size = available.min(needed);

            buf[bytes_read..bytes_read + copy_size]
                .copy_from_slice(&cluster_data[in_cluster_offset..in_cluster_offset + copy_size]);

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
            return Ok(FileInfo::new(
                String::from("/"),
                0,
                true,
                self.root_cluster as u64,
            ));
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
        self.block_size
    }

    fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        let info = self.stat(path)?;
        if info.is_dir {
            return Err(FsError::NotFile);
        }

        self.cluster_chain_extents(info.start_cluster as u32, info.size)
    }
}

impl Fat32 {
    /// Open a FAT32 filesystem from a shared block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    /// 读取簇数据
    fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>, FsError> {
        if cluster < 2 || cluster >= self.total_clusters + 2 {
            return Err(FsError::InvalidArgument);
        }

        let lba = self.cluster_to_lba(cluster);
        let mut buf = alloc_buffer(self.cluster_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), lba, &mut buf)?;
        Ok(buf)
    }

    /// 簇号转 LBA
    fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.sectors_per_cluster as u64
    }

    fn cluster_chain_extents(
        &self,
        start_cluster: u32,
        file_size: u64,
    ) -> Result<Vec<FileExtent>, FsError> {
        let mut extents = Vec::new();
        if file_size == 0 {
            return Ok(extents);
        }

        if start_cluster < 2 || start_cluster >= self.total_clusters + 2 {
            return Err(FsError::Corrupted);
        }

        let blocks_per_cluster = self.sectors_per_cluster as u64;
        let mut blocks_remaining =
            (file_size + self.block_size as u64 - 1) / self.block_size as u64;
        let mut virtual_block = 0u64;
        let mut cluster = start_cluster;

        while blocks_remaining > 0 {
            if cluster < 2 || cluster >= self.total_clusters + 2 {
                return Err(FsError::Corrupted);
            }

            let block_count = blocks_per_cluster.min(blocks_remaining);
            push_extent(
                &mut extents,
                virtual_block,
                self.cluster_to_lba(cluster),
                block_count,
            );

            virtual_block += block_count;
            blocks_remaining -= block_count;

            if blocks_remaining > 0 {
                cluster = self.get_next_cluster(cluster)?;
                if self.is_end_of_chain(cluster) {
                    return Err(FsError::Corrupted);
                }
            }
        }

        Ok(extents)
    }

    /// 获取下一个簇号
    fn get_next_cluster(&self, cluster: u32) -> Result<u32, FsError> {
        // 检查缓存
        if let Some(&next) = self.fat_cache.get(&cluster) {
            return Ok(next);
        }

        // FAT32 每个 FAT 条目 4 字节
        let fat_offset = (cluster as u64) * 4;
        let fat_sector = self.fat_start + fat_offset / self.block_size as u64;
        let fat_offset_in_sector = (fat_offset % self.block_size as u64) as usize;

        let mut sector = alloc_buffer(self.block_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), fat_sector, &mut sector)?;

        if fat_offset_in_sector + 4 > sector.len() {
            return Err(FsError::Corrupted);
        }

        let next = u32::from_le_bytes([
            sector[fat_offset_in_sector],
            sector[fat_offset_in_sector + 1],
            sector[fat_offset_in_sector + 2],
            sector[fat_offset_in_sector + 3],
        ]) & 0x0FFFFFFF;

        Ok(next)
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

    /// 检查是否为链结束
    fn is_end_of_chain(&self, cluster: u32) -> bool {
        cluster >= 0x0FFFFFF8
    }
}

fn push_extent(
    extents: &mut Vec<FileExtent>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    if block_count == 0 {
        return;
    }

    if let Some(last) = extents.last_mut() {
        if last.virtual_block_end() == virtual_block_start
            && last.physical_lba_end() == physical_lba
        {
            last.block_count += block_count;
            return;
        }
    }

    extents.push(FileExtent::new(
        virtual_block_start,
        physical_lba,
        block_count,
    ));
}

/// 检查是否为有效的 FAT32 文件系统
pub fn is_fat32(data: &[u8]) -> bool {
    if data.len() < 90 {
        return false;
    }

    // 检查引导签名
    if data[510] != 0x55 || data[511] != 0xAA {
        return false;
    }

    // 检查 FAT32 签名
    &data[0x52..0x56] == b"FAT32"
}

/// FAT32 文件系统信息
#[derive(Debug, Clone)]
pub struct Fat32Info {
    pub volume_label: String,
    pub total_size: u64,
    pub cluster_size: u32,
    pub total_clusters: u32,
}
