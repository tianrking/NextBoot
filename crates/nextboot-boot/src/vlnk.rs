//! Ventoy `.vlnk.*` pointer files.

use alloc::string::{String, ToString};

pub const VLNK_FILE_LEN: usize = 32 * 1024;
pub const VLNK_RECORD_LEN: usize = 512;
pub const VLNK_NAME_MAX: usize = 384;

const VLNK_GUID: [u8; 16] = [
    0x20, 0x20, 0x77, 0x77, 0x77, 0x2e, 0x76, 0x65, 0x6e, 0x74, 0x6f, 0x79, 0x2e, 0x6e, 0x65, 0x74,
];
const VLNK_CRC_OFFSET: usize = 16;
const VLNK_DISK_SIGNATURE_OFFSET: usize = 20;
const VLNK_PART_OFFSET_OFFSET: usize = 24;
const VLNK_FILEPATH_OFFSET: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyVlnk {
    pub disk_signature: [u8; 4],
    pub part_offset_bytes: u64,
    pub filepath: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyVlnkError {
    InvalidSize,
    InvalidGuid,
    InvalidCrc,
    InvalidPath,
}

pub fn is_vlnk_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".vlnk.iso")
        || lower.ends_with(".vlnk.wim")
        || lower.ends_with(".vlnk.img")
        || lower.ends_with(".vlnk.vhd")
        || lower.ends_with(".vlnk.efi")
        || lower.ends_with(".vlnk.dat")
        || lower.ends_with(".vlnk.vhdx")
        || lower.ends_with(".vlnk.vtoy")
}

pub fn target_image_format_path(path: &str) -> &str {
    let Some(index) = path.to_ascii_lowercase().rfind(".vlnk.") else {
        return path;
    };
    let suffix = path.get(index + ".vlnk".len()..).unwrap_or("");
    if suffix.is_empty() {
        path
    } else {
        suffix
    }
}

pub fn parse_vlnk(data: &[u8]) -> Result<VentoyVlnk, VentoyVlnkError> {
    if data.len() != VLNK_FILE_LEN {
        return Err(VentoyVlnkError::InvalidSize);
    }
    let record = data
        .get(..VLNK_RECORD_LEN)
        .ok_or(VentoyVlnkError::InvalidSize)?;
    if record.get(..VLNK_GUID.len()) != Some(&VLNK_GUID) {
        return Err(VentoyVlnkError::InvalidGuid);
    }

    let read_crc = read_u32(record, VLNK_CRC_OFFSET).ok_or(VentoyVlnkError::InvalidSize)?;
    let mut crc_record = [0u8; VLNK_RECORD_LEN];
    crc_record.copy_from_slice(record);
    crc_record[VLNK_CRC_OFFSET..VLNK_CRC_OFFSET + 4].fill(0);
    let calc_crc = crc32c(0, &crc_record);
    if read_crc != calc_crc {
        return Err(VentoyVlnkError::InvalidCrc);
    }

    let mut disk_signature = [0u8; 4];
    disk_signature.copy_from_slice(
        record
            .get(VLNK_DISK_SIGNATURE_OFFSET..VLNK_DISK_SIGNATURE_OFFSET + 4)
            .ok_or(VentoyVlnkError::InvalidSize)?,
    );
    let part_offset_bytes =
        read_u64(record, VLNK_PART_OFFSET_OFFSET).ok_or(VentoyVlnkError::InvalidSize)?;
    let path = read_c_string(
        record
            .get(VLNK_FILEPATH_OFFSET..VLNK_FILEPATH_OFFSET + VLNK_NAME_MAX)
            .ok_or(VentoyVlnkError::InvalidSize)?,
    )?;

    Ok(VentoyVlnk {
        disk_signature,
        part_offset_bytes,
        filepath: path.to_string(),
    })
}

#[cfg(test)]
pub fn build_vlnk_for_test(
    disk_signature: [u8; 4],
    part_offset_bytes: u64,
    filepath: &str,
) -> [u8; VLNK_FILE_LEN] {
    let mut data = [0u8; VLNK_FILE_LEN];
    data[..VLNK_GUID.len()].copy_from_slice(&VLNK_GUID);
    data[VLNK_DISK_SIGNATURE_OFFSET..VLNK_DISK_SIGNATURE_OFFSET + 4]
        .copy_from_slice(&disk_signature);
    data[VLNK_PART_OFFSET_OFFSET..VLNK_PART_OFFSET_OFFSET + 8]
        .copy_from_slice(&part_offset_bytes.to_le_bytes());
    let path_bytes = filepath.as_bytes();
    let copy_len = path_bytes.len().min(VLNK_NAME_MAX.saturating_sub(1));
    data[VLNK_FILEPATH_OFFSET..VLNK_FILEPATH_OFFSET + copy_len]
        .copy_from_slice(&path_bytes[..copy_len]);
    let crc = crc32c(0, &data[..VLNK_RECORD_LEN]);
    data[VLNK_CRC_OFFSET..VLNK_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    data
}

fn read_c_string(data: &[u8]) -> Result<&str, VentoyVlnkError> {
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    if end == 0 {
        return Err(VentoyVlnkError::InvalidPath);
    }
    core::str::from_utf8(&data[..end]).map_err(|_| VentoyVlnkError::InvalidPath)
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn crc32c(crc: u32, data: &[u8]) -> u32 {
    let mut crc = crc ^ 0xffff_ffff;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    crc ^ 0xffff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vlnk_pointer_with_ventoy_crc32c() {
        let data = build_vlnk_for_test([0x44, 0x33, 0x22, 0x11], 2048 * 512, "/ISO/win11.iso");

        let parsed = parse_vlnk(&data).expect("vlnk");

        assert_eq!(parsed.disk_signature, [0x44, 0x33, 0x22, 0x11]);
        assert_eq!(parsed.part_offset_bytes, 2048 * 512);
        assert_eq!(parsed.filepath, "/ISO/win11.iso");
    }

    #[test]
    fn rejects_tampered_crc() {
        let mut data = build_vlnk_for_test([1, 2, 3, 4], 4096, "/a.iso");
        data[VLNK_FILEPATH_OFFSET] = b'b';

        let err = parse_vlnk(&data).expect_err("crc");

        assert_eq!(err, VentoyVlnkError::InvalidCrc);
    }

    #[test]
    fn recognizes_ventoy_vlnk_suffixes() {
        assert!(is_vlnk_name("/ISO/foo.vlnk.iso"));
        assert!(is_vlnk_name("/ISO/foo.VLNK.VHDX"));
        assert!(is_vlnk_name("/ISO/foo.vlnk.vtoy"));
        assert!(!is_vlnk_name("/ISO/foo.iso"));
        assert_eq!(target_image_format_path("/ISO/foo.vlnk.vhdx"), ".vhdx");
    }
}
