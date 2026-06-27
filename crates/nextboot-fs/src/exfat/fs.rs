use crate::{
    alloc_buffer, read_full_blocks, FileExtent, FileInfo, FileSystem, FileSystemType, FsError,
    SharedBlockIo,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::model::ExFatBootSector;

/// exFAT 文件系统
pub struct ExFat {
    /// 底层块设备
    pub(super) block_io: SharedBlockIo,
    /// 扇区大小 (字节)
    pub(super) sector_size: u32,
    /// 簇大小 (字节)
    pub(super) cluster_size: u64,
    /// 总簇数
    pub(super) total_clusters: u32,
    /// 根目录簇号
    pub(super) root_cluster: u32,
    /// FAT 起始扇区
    pub(super) fat_offset: u64,
    /// 簇堆起始扇区
    pub(super) cluster_heap_offset: u64,
    /// 分区偏移 (字节)
    pub(super) partition_offset: u64,
    /// 卷序列号
    pub(super) volume_serial: u32,
    /// FAT 缓存
    pub(super) fat_cache: BTreeMap<u32, u32>,
}

impl FileSystem for ExFat {
    const FS_TYPE: FileSystemType = FileSystemType::ExFat;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        let mut boot_buf = alloc_buffer(block_io.block_size() as usize)?;
        read_full_blocks(block_io.as_ref(), 0, &mut boot_buf)?;

        let boot: ExFatBootSector =
            unsafe { core::ptr::read_unaligned(boot_buf.as_ptr() as *const ExFatBootSector) };

        // 验证 exFAT 签名
        if &boot_buf[3..11] != b"EXFAT   " {
            return Err(FsError::InvalidSignature);
        }

        // 检查引导签名
        if boot.signature != 0xAA55 {
            return Err(FsError::InvalidSignature);
        }

        let sector_size = 1u32 << boot.bytes_per_sector_shift;
        if sector_size != block_io.block_size() {
            return Err(FsError::BlockSizeMismatch);
        }

        let cluster_size = (sector_size as u64) << boot.sectors_per_cluster_shift;

        Ok(Self {
            block_io,
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

        let extents = if info.contiguous {
            self.contiguous_extents(info.start_cluster as u32, info.size)?
        } else {
            self.cluster_chain_extents(info.start_cluster as u32, info.size)?
        };

        self.read_from_extents(&extents, offset, &mut buf[..to_read])
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
        self.sector_size
    }

    fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        let info = self.stat(path)?;
        if info.is_dir {
            return Err(FsError::NotFile);
        }

        if info.contiguous {
            self.contiguous_extents(info.start_cluster as u32, info.size)
        } else {
            self.cluster_chain_extents(info.start_cluster as u32, info.size)
        }
    }
}

impl ExFat {
    /// Open an exFAT filesystem from a shared block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }

    /// 簇号转扇区号
    pub(super) fn cluster_to_sector(&self, cluster: u32) -> u64 {
        self.cluster_heap_offset as u64
            + ((cluster - 2) as u64) * (self.cluster_size / self.sector_size as u64)
    }

    pub(super) fn blocks_per_cluster(&self) -> u64 {
        self.cluster_size / self.sector_size as u64
    }

    /// 读取簇数据
    pub(super) fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>, FsError> {
        if cluster < 2 || cluster >= self.total_clusters + 2 {
            return Err(FsError::InvalidArgument);
        }

        let mut buf = alloc_buffer(self.cluster_size as usize)?;
        read_full_blocks(
            self.block_io.as_ref(),
            self.cluster_to_sector(cluster),
            &mut buf,
        )?;
        Ok(buf)
    }

    /// 获取下一个簇号
    pub(super) fn get_next_cluster(&self, cluster: u32) -> Result<u32, FsError> {
        if let Some(&next) = self.fat_cache.get(&cluster) {
            return Ok(next);
        }

        // exFAT FAT 条目是 32 位
        let entry_offset = (cluster as u64) * 4;
        let fat_sector = self.fat_offset + entry_offset / self.sector_size as u64;
        let fat_offset_in_sector = (entry_offset % self.sector_size as u64) as usize;

        let mut sector = alloc_buffer(self.sector_size as usize)?;
        read_full_blocks(self.block_io.as_ref(), fat_sector, &mut sector)?;

        if fat_offset_in_sector + 4 > sector.len() {
            return Err(FsError::Corrupted);
        }

        Ok(u32::from_le_bytes([
            sector[fat_offset_in_sector],
            sector[fat_offset_in_sector + 1],
            sector[fat_offset_in_sector + 2],
            sector[fat_offset_in_sector + 3],
        ]))
    }

    /// 检查是否为链结束
    pub(super) fn is_end_of_chain(&self, cluster: u32) -> bool {
        cluster >= 0xFFFFFFF8
    }
}

/// 检查是否为有效的 exFAT 文件系统
pub fn is_exfat(data: &[u8]) -> bool {
    if data.len() < 11 {
        return false;
    }

    // exFAT 跳转指令和签名
    data[0] == 0xEB && data[1] == 0x76 && data[2] == 0x90 && &data[3..11] == b"EXFAT   "
}

/// exFAT 文件系统信息
#[derive(Debug, Clone)]
pub struct ExFatInfo {
    pub volume_serial: u32,
    pub total_size: u64,
    pub cluster_size: u64,
    pub total_clusters: u32,
}
