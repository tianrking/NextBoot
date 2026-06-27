use crate::{
    alloc_buffer, read_full_blocks, FileExtent, FileInfo, FileSystem, FileSystemType, FsError,
    SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub struct Iso9660 {
    /// 底层块设备
    pub(super) block_io: SharedBlockIo,
    /// 逻辑块大小
    pub(super) block_size: u32,
    /// 卷大小 (块数)
    pub(super) volume_size: u64,
    /// 根目录 LBA
    pub(super) root_lba: u32,
    /// 根目录大小
    pub(super) root_size: u32,
    /// 卷标识
    pub(super) volume_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoDirectoryRecordLocation {
    pub record_offset: u64,
    pub extent_lba: u32,
    pub data_length: u32,
    pub is_dir: bool,
}

/// ISO9660 卷描述符
#[repr(C, packed)]
struct VolumeDescriptor {
    type_code: u8,
    standard_id: [u8; 5],
    version: u8,
    // data: [u8; 2041],
}

/// ISO9660 主卷描述符
#[repr(C, packed)]
struct PrimaryVolumeDescriptor {
    type_code: u8,
    standard_id: [u8; 5],
    version: u8,
    system_use: [u8; 32],
    volume_id: [u8; 32],
    reserved1: [u8; 8],
    volume_space_size_be: u32,
    volume_space_size_le: u32,
    reserved2: [u8; 32],
    volume_set_size_be: u16,
    volume_set_size_le: u16,
    volume_seq_num_be: u16,
    volume_seq_num_le: u16,
    logical_block_size_be: u16,
    logical_block_size_le: u16,
    path_table_size_be: u32,
    path_table_size_le: u32,
    path_table_lba_be: u32,
    path_table_lba_opt_be: u32,
    path_table_lba_le: u32,
    path_table_lba_opt_le: u32,
    root_directory: [u8; 34],
}

/// 目录记录头
#[derive(Debug, Clone)]
struct DirectoryRecordHeader {
    length: u8,
    ext_attr_length: u8,
    extent_lba: u32,
    data_length: u32,
    flags: u8,
    file_unit_size: u8,
    interleave_gap: u8,
    name_length: u8,
}

impl FileSystem for Iso9660 {
    const FS_TYPE: FileSystemType = FileSystemType::Iso9660;

    fn init(block_io: SharedBlockIo) -> Result<Self, FsError> {
        // ISO9660 卷描述符从 LBA 16 开始
        let mut vd_buf = alloc_buffer(2048)?;
        if block_io.block_size() != 2048 {
            return Err(FsError::BlockSizeMismatch);
        }

        // 扫描卷描述符
        for lba in 16..100 {
            read_full_blocks(block_io.as_ref(), lba, &mut vd_buf)?;

            // 检查标准 ID
            if &vd_buf[1..6] != b"CD001" {
                continue;
            }

            let type_code = vd_buf[0];

            // Type 1 = 主卷描述符
            if type_code == 1 {
                let logical_block_size = u16::from_le_bytes([vd_buf[128], vd_buf[129]]) as u32;
                let volume_space_size =
                    u32::from_le_bytes([vd_buf[84], vd_buf[85], vd_buf[86], vd_buf[87]]) as u64;

                // 解析卷标识
                let volume_id = String::from_utf8_lossy(&vd_buf[40..72])
                    .trim_end()
                    .to_string();

                // 解析根目录记录 (偏移 156)
                let root_lba =
                    u32::from_le_bytes([vd_buf[158], vd_buf[159], vd_buf[160], vd_buf[161]]);
                let root_size =
                    u32::from_le_bytes([vd_buf[166], vd_buf[167], vd_buf[168], vd_buf[169]]);

                return Ok(Self {
                    block_io,
                    block_size: logical_block_size,
                    volume_size: volume_space_size,
                    root_lba,
                    root_size,
                    volume_id,
                });
            }

            // Type 255 = 卷描述符集结束
            if type_code == 255 {
                break;
            }
        }

        Err(FsError::InvalidSignature)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        let info = if path == "/" || path.is_empty() {
            FileInfo::new(
                String::from("/"),
                self.root_size as u64,
                true,
                self.root_lba as u64,
            )
        } else {
            self.stat(path)?
        };

        if !info.is_dir {
            return Err(FsError::NotDirectory);
        }

        self.read_directory(info.start_cluster as u32, info.size)
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

        let start_lba = info.start_cluster as u64;
        let block_size = self.block_size as u64;

        // 计算起始位置
        let start_block = offset / block_size;
        let in_block_offset = (offset % block_size) as usize;

        let mut bytes_read = 0;
        let mut current_block = start_block;
        let mut block_buf = alloc_buffer(self.block_size as usize)?;

        while bytes_read < to_read {
            let lba = start_lba + current_block;
            read_full_blocks(self.block_io.as_ref(), lba, &mut block_buf)?;

            let available = block_buf.len() - if bytes_read == 0 { in_block_offset } else { 0 };
            let needed = to_read - bytes_read;
            let copy_size = available.min(needed);

            let src_offset = if bytes_read == 0 { in_block_offset } else { 0 };
            buf[bytes_read..bytes_read + copy_size]
                .copy_from_slice(&block_buf[src_offset..src_offset + copy_size]);

            bytes_read += copy_size;
            current_block += 1;

            // 检查是否读完
            if bytes_read >= to_read {
                break;
            }
        }

        Ok(bytes_read)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        if path == "/" || path.is_empty() {
            return Ok(FileInfo::new(
                String::from("/"),
                self.root_size as u64,
                true,
                self.root_lba as u64,
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

        let block_count = (info.size + self.block_size as u64 - 1) / self.block_size as u64;
        if block_count == 0 {
            return Ok(Vec::new());
        }

        Ok(alloc::vec![FileExtent::new(
            0,
            info.start_cluster,
            block_count,
        )])
    }
}

impl Iso9660 {
    /// Open an ISO9660 filesystem from a shared 2048-byte block device.
    pub fn open(block_io: SharedBlockIo) -> Result<Self, FsError> {
        <Self as FileSystem>::init(block_io)
    }
}
