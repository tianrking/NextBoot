//! ISO9660 文件系统实现
//!
//! 用于解析 ISO 镜像内部结构

use crate::{FileSystem, FileSystemType, FileInfo, FsError, BlockIoOps};
use alloc::vec::Vec;
use alloc::string::String;

/// ISO9660 文件系统
pub struct Iso9660 {
    block_size: u32,
    volume_size: u64,
    root_lba: u32,
}

/// ISO9660 卷描述符
#[repr(C, packed)]
struct VolumeDescriptor {
    type_code: u8,
    standard_id: [u8; 5],
    version: u8,
    data: [u8; 2041],
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
    volume_space_size: u32,  // both-endian
    reserved2: [u8; 32],
    volume_set_size: u16,
    volume_seq_num: u16,
    logical_block_size: u16,
    path_table_size: u32,
    path_table_lba: u32,
    root_directory: DirectoryEntry,
}

/// 目录记录
#[repr(C, packed)]
struct DirectoryEntry {
    length: u8,
    ext_attr_length: u8,
    extent_lba: u32,      // both-endian
    data_length: u32,     // both-endian
    date_time: [u8; 7],
    flags: u8,
    file_unit_size: u8,
    interleave_gap: u8,
    volume_seq_num: u16,
    name_length: u8,
}

impl FileSystem for Iso9660 {
    const FS_TYPE: FileSystemType = FileSystemType::Iso9660;

    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError> {
        // ISO9660 卷描述符从 LBA 16 开始
        let mut vd_buf = [0u8; 2048];

        // 扫描卷描述符
        for lba in 16..100 {
            block_io.read_blocks(lba, &mut vd_buf)?;

            let vd: VolumeDescriptor = unsafe {
                core::mem::transmute_copy(&vd_buf)
            };

            // 检查标准 ID
            if &vd.standard_id != b"CD001" {
                continue;
            }

            // Type 1 = 主卷描述符
            if vd.type_code == 1 {
                let pvd: PrimaryVolumeDescriptor = unsafe {
                    core::mem::transmute_copy(&vd_buf)
                };

                return Ok(Self {
                    block_size: pvd.logical_block_size as u32,
                    volume_size: pvd.volume_space_size as u64,
                    root_lba: pvd.root_directory.extent_lba,
                });
            }

            // Type 255 = 卷描述符集结束
            if vd.type_code == 255 {
                break;
            }
        }

        Err(FsError::InvalidSignature)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError> {
        // TODO: 实现 ISO9660 目录读取
        Ok(Vec::new())
    }

    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        // TODO: 实现文件读取
        Ok(0)
    }

    fn stat(&self, path: &str) -> Result<FileInfo, FsError> {
        Err(FsError::FileNotFound)
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }
}

/// 检测 ISO 是否为可启动
pub fn is_bootable_iso(iso_data: &[u8]) -> bool {
    // 检查 El Torito 引导记录 (LBA 11)
    if iso_data.len() < 0x8800 {
        return false;
    }

    // 验证卷描述符
    let vd = &iso_data[0x8000..];
    if &vd[1..6] != b"CD001" {
        return false;
    }

    // 检查引导记录 (type 0)
    if vd[0] == 0 {
        return true;
    }

    false
}
