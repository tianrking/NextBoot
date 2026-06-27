use crate::{alloc_buffer, read_full_blocks, FsError};

use super::{
    EL_TORITO_BOOTABLE, EL_TORITO_BOOT_RECORD_LBA, EL_TORITO_FINAL_SECTION_HEADER,
    EL_TORITO_PLATFORM_EFI, EL_TORITO_SECTION_HEADER, ISO_SECTOR_SIZE, UDF_PROBE_END_LBA,
    UDF_PROBE_START_LBA,
};

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
