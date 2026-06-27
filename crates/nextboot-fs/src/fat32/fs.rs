use crate::{
    alloc_buffer, read_full_blocks, FileExtent, FileInfo, FileSystem, FileSystemType, FsError,
    SharedBlockIo,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::model::Fat32BootSector;

/// FAT32 文件系统
pub struct Fat32 {
    /// 底层块设备
    pub(super) block_io: SharedBlockIo,
    /// 块大小
    pub(super) block_size: u32,
    /// 每簇扇区数
    pub(super) sectors_per_cluster: u8,
    /// 簇大小 (字节)
    pub(super) cluster_size: u32,
    /// FAT 表起始扇区
    pub(super) fat_start: u64,
    /// FAT 表大小 (扇区)
    pub(super) fat_size: u64,
    /// FAT 表数量
    pub(super) num_fats: u8,
    /// 数据区起始扇区
    pub(super) data_start: u64,
    /// 根目录簇号
    pub(super) root_cluster: u32,
    /// 总簇数
    pub(super) total_clusters: u32,
    /// 卷标
    pub(super) volume_label: String,
    /// FAT 缓存 (部分)
    pub(super) fat_cache: BTreeMap<u32, u32>,
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
    pub(super) fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>, FsError> {
        if cluster < 2 || cluster >= self.total_clusters + 2 {
            return Err(FsError::InvalidArgument);
        }

        let lba = self.cluster_to_lba(cluster);
        let mut buf = alloc_buffer(self.cluster_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), lba, &mut buf)?;
        Ok(buf)
    }

    /// 簇号转 LBA
    pub(super) fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.sectors_per_cluster as u64
    }

    /// 获取下一个簇号
    pub(super) fn get_next_cluster(&self, cluster: u32) -> Result<u32, FsError> {
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

    /// 检查是否为链结束
    pub(super) fn is_end_of_chain(&self, cluster: u32) -> bool {
        cluster >= 0x0FFFFFF8
    }
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
    data.len() >= 0x5A && data[0x52..0x5A].starts_with(b"FAT32")
}

/// FAT32 文件系统信息
#[derive(Debug, Clone)]
pub struct Fat32Info {
    pub volume_label: String,
    pub total_size: u64,
    pub cluster_size: u32,
    pub total_clusters: u32,
}
