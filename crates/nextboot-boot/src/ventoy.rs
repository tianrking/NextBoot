//! Ventoy-compatible runtime parameter builders.
//!
//! Ventoy publishes a 512-byte `VentoyOsParam` UEFI variable and, for normal
//! disk-backed images, points it at a runtime `ventoy_image_location` table.
//! Keeping the layout construction isolated makes the wire format testable
//! outside the UEFI binary.

use alloc::vec::Vec;

pub const VENTOY_OS_PARAM_NAME: &str = "VentoyOsParam";
pub const VENTOY_OS_PARAM_SIZE: usize = 512;
pub const VENTOY_IMAGE_PATH_SIZE: usize = 384;
pub const VENTOY_IMAGE_LOCATION_HEADER_SIZE: usize = 28;
pub const VENTOY_IMAGE_LOCATION_REGION_SIZE: usize = 16;
pub const VENTOY_PART_TYPE_EXFAT: u16 = 0;
pub const VENTOY_PART_TYPE_NTFS: u16 = 1;
pub const VENTOY_PART_TYPE_FAT: u16 = 5;
pub const VENTOY_PART_TYPE_OTHER: u16 = 6;
pub const VENTOY_CHAIN_LINUX: u8 = 0;
pub const VENTOY_CHAIN_WINDOWS: u8 = 1;
pub const VENTOY_CHAIN_WIM: u8 = 2;
pub const VENTOY_RESERVED_SIZE: usize = 32;

pub const VENTOY_GUID_BYTES: [u8; 16] = [
    0x20, 0x20, 0x77, 0x77, 0x77, 0x2e, 0x76, 0x65, 0x6e, 0x74, 0x6f, 0x79, 0x2e, 0x6e, 0x65, 0x74,
];

const OS_PARAM_CHECKSUM_OFFSET: usize = 16;
const OS_PARAM_DISK_GUID_OFFSET: usize = 17;
const OS_PARAM_DISK_SIZE_OFFSET: usize = 33;
const OS_PARAM_PART_ID_OFFSET: usize = 41;
const OS_PARAM_PART_TYPE_OFFSET: usize = 43;
const OS_PARAM_IMAGE_PATH_OFFSET: usize = 45;
const OS_PARAM_IMAGE_SIZE_OFFSET: usize = 429;
const OS_PARAM_IMAGE_LOCATION_ADDR_OFFSET: usize = 437;
const OS_PARAM_IMAGE_LOCATION_LEN_OFFSET: usize = 445;
const OS_PARAM_RESERVED_OFFSET: usize = 449;
const OS_PARAM_DISK_SIGNATURE_OFFSET: usize = 481;
const VENTOY_RESERVED_CHAIN_TYPE_OFFSET: usize = 2;
const VENTOY_RESERVED_ISO_FORMAT_OFFSET: usize = 3;
const VENTOY_RESERVED_WINDOWS_CD_PROMPT_OFFSET: usize = 4;
const VENTOY_RESERVED_LINUX_REMOUNT_OFFSET: usize = 5;
const VENTOY_RESERVED_VLNK_OFFSET: usize = 6;
const VENTOY_RESERVED_DISK_SIGNATURE_OFFSET: usize = 7;
const VENTOY_RESERVED_WINDOWS_MAX_RES_OFFSET: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyParamError {
    PathTooLong,
    InvalidSectorSize,
    UnalignedExtent,
    ValueOutOfRange,
    OutputTooLarge,
    OutputReserveFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyReserved {
    bytes: [u8; VENTOY_RESERVED_SIZE],
}

impl VentoyReserved {
    pub const fn new() -> Self {
        Self {
            bytes: [0; VENTOY_RESERVED_SIZE],
        }
    }

    pub fn with_chain_type(mut self, chain_type: u8) -> Self {
        self.bytes[VENTOY_RESERVED_CHAIN_TYPE_OFFSET] = chain_type;
        self
    }

    pub fn with_iso_udf(mut self, is_udf: bool) -> Self {
        self.bytes[VENTOY_RESERVED_ISO_FORMAT_OFFSET] = if is_udf { 1 } else { 0 };
        self
    }

    pub fn with_windows_cd_prompt(mut self, preserve_prompt: bool) -> Self {
        self.bytes[VENTOY_RESERVED_WINDOWS_CD_PROMPT_OFFSET] = if preserve_prompt { 1 } else { 0 };
        self
    }

    pub fn with_linux_remount(mut self, remount: bool) -> Self {
        self.bytes[VENTOY_RESERVED_LINUX_REMOUNT_OFFSET] = if remount { 1 } else { 0 };
        self
    }

    pub fn with_vlnk(mut self, enabled: bool) -> Self {
        self.bytes[VENTOY_RESERVED_VLNK_OFFSET] = if enabled { 1 } else { 0 };
        self
    }

    pub fn with_disk_signature(mut self, signature: [u8; 4]) -> Self {
        self.bytes[VENTOY_RESERVED_DISK_SIGNATURE_OFFSET] = signature[0];
        self.bytes[VENTOY_RESERVED_DISK_SIGNATURE_OFFSET + 1] = signature[1];
        self.bytes[VENTOY_RESERVED_DISK_SIGNATURE_OFFSET + 2] = signature[2];
        self.bytes[VENTOY_RESERVED_DISK_SIGNATURE_OFFSET + 3] = signature[3];
        self
    }

    pub fn with_windows_max_resolution(mut self, mode: u8) -> Self {
        self.bytes[VENTOY_RESERVED_WINDOWS_MAX_RES_OFFSET] = mode;
        self
    }

    pub fn as_bytes(&self) -> &[u8; VENTOY_RESERVED_SIZE] {
        &self.bytes
    }
}

impl Default for VentoyReserved {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyOsParamInput<'a> {
    pub disk_guid: [u8; 16],
    pub disk_size: u64,
    pub disk_part_id: u16,
    pub disk_part_type: u16,
    pub image_path: &'a str,
    pub image_size: u64,
    pub image_location_addr: u64,
    pub image_location_len: u32,
    pub reserved: VentoyReserved,
    pub disk_signature: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyExtent {
    pub virtual_block_start: u64,
    pub physical_lba: u64,
    pub block_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyImageRegion {
    pub image_sector_count: u32,
    pub image_start_sector: u32,
    pub disk_start_sector: u64,
}

pub fn build_ventoy_os_param(
    input: &VentoyOsParamInput<'_>,
) -> Result<[u8; VENTOY_OS_PARAM_SIZE], VentoyParamError> {
    let path = input.image_path.as_bytes();
    if path.len() > VENTOY_IMAGE_PATH_SIZE {
        return Err(VentoyParamError::PathTooLong);
    }

    let mut data = [0u8; VENTOY_OS_PARAM_SIZE];
    data[..VENTOY_GUID_BYTES.len()].copy_from_slice(&VENTOY_GUID_BYTES);
    data[OS_PARAM_DISK_GUID_OFFSET..OS_PARAM_DISK_GUID_OFFSET + input.disk_guid.len()]
        .copy_from_slice(&input.disk_guid);
    write_u64(&mut data, OS_PARAM_DISK_SIZE_OFFSET, input.disk_size);
    write_u16(&mut data, OS_PARAM_PART_ID_OFFSET, input.disk_part_id);
    write_u16(&mut data, OS_PARAM_PART_TYPE_OFFSET, input.disk_part_type);
    data[OS_PARAM_IMAGE_PATH_OFFSET..OS_PARAM_IMAGE_PATH_OFFSET + path.len()].copy_from_slice(path);
    write_u64(&mut data, OS_PARAM_IMAGE_SIZE_OFFSET, input.image_size);
    write_u64(
        &mut data,
        OS_PARAM_IMAGE_LOCATION_ADDR_OFFSET,
        input.image_location_addr,
    );
    write_u32(
        &mut data,
        OS_PARAM_IMAGE_LOCATION_LEN_OFFSET,
        input.image_location_len,
    );
    data[OS_PARAM_RESERVED_OFFSET..OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_SIZE]
        .copy_from_slice(input.reserved.as_bytes());
    data[OS_PARAM_DISK_SIGNATURE_OFFSET..OS_PARAM_DISK_SIGNATURE_OFFSET + 4]
        .copy_from_slice(&input.disk_signature);

    data[OS_PARAM_CHECKSUM_OFFSET] = 0;
    let checksum = data
        .iter()
        .copied()
        .fold(0u8, |sum, byte| sum.wrapping_add(byte));
    data[OS_PARAM_CHECKSUM_OFFSET] = 0u8.wrapping_sub(checksum);

    Ok(data)
}

pub fn build_ventoy_image_regions(
    extents: &[VentoyExtent],
    source_block_size: u32,
    image_sector_size: u32,
) -> Result<Vec<VentoyImageRegion>, VentoyParamError> {
    if source_block_size == 0 || image_sector_size == 0 {
        return Err(VentoyParamError::InvalidSectorSize);
    }

    let image_sector_size = u64::from(image_sector_size);
    let source_block_size = u64::from(source_block_size);
    let mut regions = Vec::new();
    regions
        .try_reserve_exact(extents.len())
        .map_err(|_| VentoyParamError::OutputReserveFailed)?;

    for extent in extents {
        let image_start_bytes = extent
            .virtual_block_start
            .checked_mul(source_block_size)
            .ok_or(VentoyParamError::ValueOutOfRange)?;
        let image_bytes = extent
            .block_count
            .checked_mul(source_block_size)
            .ok_or(VentoyParamError::ValueOutOfRange)?;
        if image_start_bytes % image_sector_size != 0 || image_bytes % image_sector_size != 0 {
            return Err(VentoyParamError::UnalignedExtent);
        }

        let image_start_sector = image_start_bytes / image_sector_size;
        let image_sector_count = image_bytes / image_sector_size;
        regions.push(VentoyImageRegion {
            image_sector_count: u32::try_from(image_sector_count)
                .map_err(|_| VentoyParamError::ValueOutOfRange)?,
            image_start_sector: u32::try_from(image_start_sector)
                .map_err(|_| VentoyParamError::ValueOutOfRange)?,
            disk_start_sector: extent.physical_lba,
        });
    }

    Ok(regions)
}

pub fn build_ventoy_image_location(
    image_sector_size: u32,
    disk_sector_size: u32,
    regions: &[VentoyImageRegion],
) -> Result<Vec<u8>, VentoyParamError> {
    if image_sector_size == 0 || disk_sector_size == 0 {
        return Err(VentoyParamError::InvalidSectorSize);
    }
    let total_size = VENTOY_IMAGE_LOCATION_HEADER_SIZE
        .checked_add(
            regions
                .len()
                .checked_mul(VENTOY_IMAGE_LOCATION_REGION_SIZE)
                .ok_or(VentoyParamError::OutputTooLarge)?,
        )
        .ok_or(VentoyParamError::OutputTooLarge)?;

    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| VentoyParamError::OutputReserveFailed)?;
    data.extend_from_slice(&VENTOY_GUID_BYTES);
    push_u32(&mut data, image_sector_size);
    push_u32(&mut data, disk_sector_size);
    push_u32(
        &mut data,
        u32::try_from(regions.len()).map_err(|_| VentoyParamError::ValueOutOfRange)?,
    );

    for region in regions {
        push_u32(&mut data, region.image_sector_count);
        push_u32(&mut data, region.image_start_sector);
        push_u64(&mut data, region.disk_start_sector);
    }

    debug_assert_eq!(data.len(), total_size);
    Ok(data)
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_os_param_with_checksum_and_expected_offsets() {
        let reserved = VentoyReserved::new()
            .with_chain_type(VENTOY_CHAIN_WINDOWS)
            .with_iso_udf(false)
            .with_windows_cd_prompt(false)
            .with_disk_signature([0xaa, 0xbb, 0xcc, 0xdd])
            .with_windows_max_resolution(2);
        let input = VentoyOsParamInput {
            disk_guid: [0x11; 16],
            disk_size: 0x1122_3344_5566_7788,
            disk_part_id: 2,
            disk_part_type: VENTOY_PART_TYPE_EXFAT,
            image_path: "/iso/win11.iso",
            image_size: 0x8877_6655_4433_2211,
            image_location_addr: 0x1234_5000,
            image_location_len: 44,
            reserved,
            disk_signature: [0xaa, 0xbb, 0xcc, 0xdd],
        };

        let data = build_ventoy_os_param(&input).expect("os param");

        assert_eq!(data.len(), VENTOY_OS_PARAM_SIZE);
        assert_eq!(&data[..16], &VENTOY_GUID_BYTES);
        assert_eq!(
            data.iter()
                .copied()
                .fold(0u8, |sum, byte| sum.wrapping_add(byte)),
            0
        );
        assert_eq!(
            &data[OS_PARAM_DISK_GUID_OFFSET..OS_PARAM_DISK_GUID_OFFSET + 16],
            &[0x11; 16]
        );
        assert_eq!(
            &data[OS_PARAM_IMAGE_PATH_OFFSET..OS_PARAM_IMAGE_PATH_OFFSET + 14],
            b"/iso/win11.iso"
        );
        assert_eq!(
            u64::from_le_bytes(
                data[OS_PARAM_IMAGE_LOCATION_ADDR_OFFSET..OS_PARAM_IMAGE_LOCATION_ADDR_OFFSET + 8]
                    .try_into()
                    .unwrap()
            ),
            0x1234_5000
        );
        assert_eq!(
            &data[OS_PARAM_DISK_SIGNATURE_OFFSET..OS_PARAM_DISK_SIGNATURE_OFFSET + 4],
            &[0xaa, 0xbb, 0xcc, 0xdd]
        );
        assert_eq!(
            data[OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_CHAIN_TYPE_OFFSET],
            VENTOY_CHAIN_WINDOWS
        );
        assert_eq!(
            data[OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_ISO_FORMAT_OFFSET],
            0
        );
        assert_eq!(
            data[OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_WINDOWS_CD_PROMPT_OFFSET],
            0
        );
        assert_eq!(
            &data[OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_DISK_SIGNATURE_OFFSET
                ..OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_DISK_SIGNATURE_OFFSET + 4],
            &[0xaa, 0xbb, 0xcc, 0xdd]
        );
        assert_eq!(
            data[OS_PARAM_RESERVED_OFFSET + VENTOY_RESERVED_WINDOWS_MAX_RES_OFFSET],
            2
        );
    }

    #[test]
    fn reserved_flags_match_ventoy_byte_offsets() {
        let reserved = VentoyReserved::new()
            .with_chain_type(VENTOY_CHAIN_WIM)
            .with_iso_udf(true)
            .with_windows_cd_prompt(true)
            .with_linux_remount(true)
            .with_vlnk(true)
            .with_disk_signature([1, 2, 3, 4])
            .with_windows_max_resolution(3);

        assert_eq!(reserved.as_bytes()[2], VENTOY_CHAIN_WIM);
        assert_eq!(reserved.as_bytes()[3], 1);
        assert_eq!(reserved.as_bytes()[4], 1);
        assert_eq!(reserved.as_bytes()[5], 1);
        assert_eq!(reserved.as_bytes()[6], 1);
        assert_eq!(&reserved.as_bytes()[7..11], &[1, 2, 3, 4]);
        assert_eq!(reserved.as_bytes()[11], 3);
    }

    #[test]
    fn builds_image_location_from_4k_extents() {
        let extents = [VentoyExtent {
            virtual_block_start: 2,
            physical_lba: 100,
            block_count: 3,
        }];
        let regions = build_ventoy_image_regions(&extents, 4096, 2048).expect("regions");
        let data = build_ventoy_image_location(2048, 4096, &regions).expect("location");

        assert_eq!(regions[0].image_start_sector, 4);
        assert_eq!(regions[0].image_sector_count, 6);
        assert_eq!(regions[0].disk_start_sector, 100);
        assert_eq!(
            data.len(),
            VENTOY_IMAGE_LOCATION_HEADER_SIZE + VENTOY_IMAGE_LOCATION_REGION_SIZE
        );
        assert_eq!(&data[..16], &VENTOY_GUID_BYTES);
        assert_eq!(u32::from_le_bytes(data[16..20].try_into().unwrap()), 2048);
        assert_eq!(u32::from_le_bytes(data[20..24].try_into().unwrap()), 4096);
        assert_eq!(u32::from_le_bytes(data[24..28].try_into().unwrap()), 1);
    }

    #[test]
    fn rejects_unaligned_2048_regions() {
        let extents = [VentoyExtent {
            virtual_block_start: 1,
            physical_lba: 8,
            block_count: 1,
        }];

        let err = build_ventoy_image_regions(&extents, 512, 2048).expect_err("unaligned");

        assert_eq!(err, VentoyParamError::UnalignedExtent);
    }
}
