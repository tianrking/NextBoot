//! ISO9660 文件系统实现
//!
//! 用于解析 ISO 镜像内部结构

use crate::{
    alloc_buffer, read_full_blocks, FileAttributes, FileExtent, FileInfo, FileSystem,
    FileSystemType, FsError, SharedBlockIo,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const ISO_SECTOR_SIZE: usize = 2048;
const EL_TORITO_BOOT_RECORD_LBA: u64 = 17;
const EL_TORITO_PLATFORM_EFI: u8 = 0xEF;
const EL_TORITO_BOOTABLE: u8 = 0x88;
const EL_TORITO_SECTION_HEADER: u8 = 0x90;
const EL_TORITO_FINAL_SECTION_HEADER: u8 = 0x91;
const UDF_PROBE_START_LBA: u64 = 16;
const UDF_PROBE_END_LBA: u64 = 32;

/// Parsed El Torito boot catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElToritoBootInfo {
    /// ISO9660 LBA containing the boot catalog.
    pub catalog_lba: u32,
    /// Boot catalog entry number used by UEFI CD-ROM device paths.
    pub boot_entry: u32,
    /// El Torito platform id. `0xEF` means EFI.
    pub platform_id: u8,
    /// El Torito boot media type.
    pub boot_media_type: u8,
    /// ISO9660 LBA containing the boot image.
    pub image_lba: u32,
    /// Boot catalog sector count, expressed as 512-byte sectors by El Torito.
    pub sector_count: u16,
}

impl ElToritoBootInfo {
    pub fn image_block_count_2048(&self) -> u64 {
        let byte_count = u64::from(self.sector_count).saturating_mul(512);
        ((byte_count + 2047) / 2048).max(1)
    }

    pub fn is_efi(&self) -> bool {
        self.platform_id == EL_TORITO_PLATFORM_EFI
    }
}

/// ISO9660 文件系统
pub struct Iso9660 {
    /// 底层块设备
    block_io: SharedBlockIo,
    /// 逻辑块大小
    block_size: u32,
    /// 卷大小 (块数)
    volume_size: u64,
    /// 根目录 LBA
    root_lba: u32,
    /// 根目录大小
    root_size: u32,
    /// 卷标识
    volume_id: String,
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

    /// 读取目录
    fn read_directory(&self, lba: u32, size: u64) -> Result<Vec<FileInfo>, FsError> {
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

/// Read the El Torito boot catalog and return the preferred UEFI entry.
pub fn read_efi_eltorito_boot_info(
    block_io: &dyn crate::BlockIoOps,
) -> Result<Option<ElToritoBootInfo>, FsError> {
    Ok(read_eltorito_boot_info(block_io)?
        .filter(|entry| entry.platform_id == EL_TORITO_PLATFORM_EFI))
}

/// Detect whether an ISO image also contains a UDF volume recognition sequence.
///
/// Ventoy uses this same descriptor order to decide whether `ventoy_fs_probe`
/// should be `udf`: scan ISO descriptors from sector 16, stop at the volume
/// descriptor terminator, then expect `BEA01` followed by `NSR02` or `NSR03`.
pub fn detect_udf_volume(block_io: &dyn crate::BlockIoOps) -> Result<bool, FsError> {
    if block_io.block_size() != ISO_SECTOR_SIZE as u32 {
        return Err(FsError::BlockSizeMismatch);
    }

    let mut sector = alloc_buffer(ISO_SECTOR_SIZE)?;
    let mut terminator_lba = None;

    for lba in UDF_PROBE_START_LBA..UDF_PROBE_END_LBA {
        if !read_iso_sector(block_io, lba, &mut sector)? {
            return Ok(false);
        }

        if sector[0] == 255 {
            terminator_lba = Some(lba);
            break;
        }
    }

    let Some(bea_lba) = terminator_lba.and_then(|lba| lba.checked_add(1)) else {
        return Ok(false);
    };
    if !read_iso_sector(block_io, bea_lba, &mut sector)? || &sector[1..6] != b"BEA01" {
        return Ok(false);
    }

    let Some(nsr_lba) = bea_lba.checked_add(1) else {
        return Ok(false);
    };
    if !read_iso_sector(block_io, nsr_lba, &mut sector)? {
        return Ok(false);
    }

    Ok(&sector[1..6] == b"NSR02" || &sector[1..6] == b"NSR03")
}

fn read_iso_sector(
    block_io: &dyn crate::BlockIoOps,
    lba: u64,
    sector: &mut [u8],
) -> Result<bool, FsError> {
    if lba >= block_io.total_blocks() {
        return Ok(false);
    }

    read_full_blocks(block_io, lba, sector)?;
    Ok(true)
}

/// Read the El Torito boot catalog and return the best available entry.
///
/// EFI section entries are preferred. If an ISO only has a default boot entry,
/// that entry is returned as a BIOS/unknown-platform fallback.
pub fn read_eltorito_boot_info(
    block_io: &dyn crate::BlockIoOps,
) -> Result<Option<ElToritoBootInfo>, FsError> {
    if block_io.block_size() != ISO_SECTOR_SIZE as u32 {
        return Err(FsError::BlockSizeMismatch);
    }

    let mut sector = alloc_buffer(ISO_SECTOR_SIZE)?;
    read_full_blocks(block_io, EL_TORITO_BOOT_RECORD_LBA, &mut sector)?;
    let Some(catalog_lba) = parse_boot_catalog_lba(&sector) else {
        return Ok(None);
    };

    read_full_blocks(block_io, u64::from(catalog_lba), &mut sector)?;
    Ok(parse_boot_catalog(catalog_lba, &sector))
}

fn parse_boot_catalog_lba(sector: &[u8]) -> Option<u32> {
    if sector.len() < ISO_SECTOR_SIZE {
        return None;
    }
    if sector[0] != 0 || sector[6] != 1 || &sector[1..6] != b"CD001" {
        return None;
    }
    if &sector[7..30] != b"EL TORITO SPECIFICATION" {
        return None;
    }

    let catalog_lba = u32::from_le_bytes([sector[0x47], sector[0x48], sector[0x49], sector[0x4A]]);
    (catalog_lba != 0).then_some(catalog_lba)
}

fn parse_boot_catalog(catalog_lba: u32, catalog: &[u8]) -> Option<ElToritoBootInfo> {
    if catalog.len() < 64 || catalog[0] != 0x01 || catalog[30] != 0x55 || catalog[31] != 0xAA {
        return None;
    }

    let validation_platform = catalog[1];
    let default_entry = parse_boot_entry(catalog_lba, 0, validation_platform, &catalog[32..64]);
    if matches!(default_entry, Some(entry) if entry.is_efi()) {
        return default_entry;
    }

    let mut offset = 64usize;
    let mut entry_index = 1u32;
    while offset + 32 <= catalog.len() {
        let header = &catalog[offset..offset + 32];
        let indicator = header[0];
        if indicator != EL_TORITO_SECTION_HEADER && indicator != EL_TORITO_FINAL_SECTION_HEADER {
            break;
        }

        let platform_id = header[1];
        let entry_count = u16::from_le_bytes([header[2], header[3]]) as usize;
        offset += 32;

        for _ in 0..entry_count {
            if offset + 32 > catalog.len() {
                return default_entry;
            }

            if platform_id == EL_TORITO_PLATFORM_EFI {
                if let Some(entry) = parse_boot_entry(
                    catalog_lba,
                    entry_index,
                    platform_id,
                    &catalog[offset..offset + 32],
                ) {
                    return Some(entry);
                }
            }

            entry_index += 1;
            offset += 32;
        }

        if indicator == EL_TORITO_FINAL_SECTION_HEADER {
            break;
        }
    }

    default_entry
}

fn parse_boot_entry(
    catalog_lba: u32,
    boot_entry: u32,
    platform_id: u8,
    entry: &[u8],
) -> Option<ElToritoBootInfo> {
    if entry.len() < 32 || entry[0] != EL_TORITO_BOOTABLE {
        return None;
    }

    let image_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
    if image_lba == 0 {
        return None;
    }

    Some(ElToritoBootInfo {
        catalog_lba,
        boot_entry,
        platform_id,
        boot_media_type: entry[1],
        image_lba,
        sector_count: u16::from_le_bytes([entry[6], entry[7]]),
    })
}

/// 检测 ISO 是否为可启动
pub fn is_bootable_iso(data: &[u8]) -> bool {
    if data.len() < 0x8800 {
        return false;
    }

    // 验证卷描述符
    let vd = &data[0x8000..];
    if &vd[1..6] != b"CD001" {
        return false;
    }

    // 检查引导记录 (type 0) 或主卷描述符 (type 1)
    vd[0] == 0 || vd[0] == 1
}

/// El Torito 引导记录
#[repr(C, packed)]
struct ElToritoBootRecord {
    type_code: u8,
    standard_id: [u8; 5],
    version: u8,
    boot_system_id: [u8; 32],
    boot_catalog_lba: u32,
}

/// El Torito 引导目录入口
#[repr(C, packed)]
struct BootCatalogEntry {
    boot_indicator: u8,
    boot_media_type: u8,
    load_segment: u16,
    system_type: u8,
    unused1: u8,
    sector_count: u16,
    load_rba: u32,
}

/// 获取 El Torito 引导信息
pub fn get_eltorito_boot_info(data: &[u8]) -> Option<(u32, u16)> {
    // 查找引导记录卷描述符
    for lba in 16..100 {
        let offset = lba * 2048;
        if offset + 2048 > data.len() {
            break;
        }

        let vd = &data[offset..offset + 2048];
        if &vd[1..6] != b"CD001" {
            continue;
        }

        if vd[0] == 0 {
            let catalog_lba = parse_boot_catalog_lba(vd)?;

            // 读取引导目录
            let cat_offset = catalog_lba as usize * 2048;
            if cat_offset + 2048 > data.len() {
                return None;
            }

            let entry = parse_boot_catalog(catalog_lba, &data[cat_offset..cat_offset + 2048])?;
            return Some((entry.image_lba, entry.sector_count));
        }

        if vd[0] == 255 {
            break;
        }
    }

    None
}

/// 检测 ISO 中的操作系统类型
pub fn detect_os_type(files: &[&str]) -> IsoOsType {
    for file in files {
        let file_lower = file.to_lowercase();

        // Windows
        if file_lower.contains("bootmgfw.efi") || file_lower.contains("install.wim") {
            return IsoOsType::Windows;
        }

        // Ubuntu
        if file_lower.contains("casper/vmlinuz") || file_lower.contains(".disk/info") {
            return IsoOsType::Ubuntu;
        }

        // Debian
        if file_lower.contains("install.amd") {
            return IsoOsType::Debian;
        }

        // Fedora
        if file_lower.contains("images/pxeboot") {
            return IsoOsType::Fedora;
        }

        // Arch
        if file_lower.contains("arch/boot") {
            return IsoOsType::Arch;
        }

        // 通用 Linux
        if file_lower.contains("vmlinuz") || file_lower.contains("initrd") {
            return IsoOsType::GenericLinux;
        }
    }

    IsoOsType::Unknown
}

/// ISO 操作系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoOsType {
    Windows,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    GenericLinux,
    Unknown,
}
