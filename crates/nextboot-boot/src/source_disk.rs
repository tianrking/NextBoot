//! Source disk identity helpers for Ventoy-compatible OS parameters.

#[cfg(test)]
extern crate alloc;

use alloc::vec::Vec;

const DEVICE_PATH_TYPE_MEDIA: u8 = 0x04;
const DEVICE_PATH_SUBTYPE_HARD_DRIVE: u8 = 0x01;
const DEVICE_PATH_TYPE_END: u8 = 0x7f;
const DEVICE_PATH_SUBTYPE_END_ENTIRE: u8 = 0xff;
const DEVICE_PATH_END_ENTIRE: [u8; 4] = [
    DEVICE_PATH_TYPE_END,
    DEVICE_PATH_SUBTYPE_END_ENTIRE,
    0x04,
    0x00,
];
const HARD_DRIVE_DEVICE_PATH_LEN: usize = 42;
const VENTOY_DISK_GUID_OFFSET: usize = 0x180;
const VENTOY_DISK_SIGNATURE_OFFSET: usize = 0x1b8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionFormat {
    Unknown,
    Mbr,
    Gpt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardDriveDevicePathInfo {
    pub node_offset: usize,
    pub partition_number: u32,
    pub partition_start_lba: u64,
    pub partition_size_blocks: u64,
    pub partition_format: PartitionFormat,
    pub signature_type: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceDiskIdentity {
    pub disk_guid: [u8; 16],
    pub disk_signature: [u8; 4],
    pub disk_size: u64,
    pub block_size: u32,
    pub partition_number: u16,
    pub partition_start_lba: u64,
    pub partition_size_blocks: u64,
    pub partition_format: PartitionFormat,
}

pub fn parse_last_hard_drive_device_path(bytes: &[u8]) -> Option<HardDriveDevicePathInfo> {
    let mut offset = 0usize;
    let mut hard_drive = None;

    while offset.checked_add(4)? <= bytes.len() {
        let node_type = *bytes.get(offset)?;
        let node_subtype = *bytes.get(offset + 1)?;
        let node_len =
            u16::from_le_bytes([*bytes.get(offset + 2)?, *bytes.get(offset + 3)?]) as usize;
        if node_len < 4 || offset.checked_add(node_len)? > bytes.len() {
            return None;
        }

        if node_type == DEVICE_PATH_TYPE_END && node_subtype == DEVICE_PATH_SUBTYPE_END_ENTIRE {
            return hard_drive;
        }

        if node_type == DEVICE_PATH_TYPE_MEDIA
            && node_subtype == DEVICE_PATH_SUBTYPE_HARD_DRIVE
            && node_len >= HARD_DRIVE_DEVICE_PATH_LEN
        {
            hard_drive = Some(HardDriveDevicePathInfo {
                node_offset: offset,
                partition_number: read_u32(bytes, offset + 4)?,
                partition_start_lba: read_u64(bytes, offset + 8)?,
                partition_size_blocks: read_u64(bytes, offset + 16)?,
                partition_format: match *bytes.get(offset + 40)? {
                    0x01 => PartitionFormat::Mbr,
                    0x02 => PartitionFormat::Gpt,
                    _ => PartitionFormat::Unknown,
                },
                signature_type: *bytes.get(offset + 41)?,
            });
        }

        offset = offset.checked_add(node_len)?;
    }

    None
}

pub fn parent_device_path_bytes(
    bytes: &[u8],
    hard_drive: &HardDriveDevicePathInfo,
) -> Option<Vec<u8>> {
    if hard_drive.node_offset > bytes.len() {
        return None;
    }

    let mut parent = Vec::new();
    parent
        .try_reserve_exact(hard_drive.node_offset + DEVICE_PATH_END_ENTIRE.len())
        .ok()?;
    parent.extend_from_slice(bytes.get(..hard_drive.node_offset)?);
    parent.extend_from_slice(&DEVICE_PATH_END_ENTIRE);
    Some(parent)
}

pub fn build_source_disk_identity(
    first_disk_block: &[u8],
    disk_size: u64,
    block_size: u32,
    hard_drive: Option<HardDriveDevicePathInfo>,
) -> Option<SourceDiskIdentity> {
    let guid = first_disk_block.get(VENTOY_DISK_GUID_OFFSET..VENTOY_DISK_GUID_OFFSET + 16)?;
    let signature =
        first_disk_block.get(VENTOY_DISK_SIGNATURE_OFFSET..VENTOY_DISK_SIGNATURE_OFFSET + 4)?;
    let mut disk_guid = [0u8; 16];
    disk_guid.copy_from_slice(guid);
    let mut disk_signature = [0u8; 4];
    disk_signature.copy_from_slice(signature);

    let (partition_number, partition_start_lba, partition_size_blocks, partition_format) =
        if let Some(info) = hard_drive {
            (
                u16::try_from(info.partition_number).ok()?,
                info.partition_start_lba,
                info.partition_size_blocks,
                info.partition_format,
            )
        } else {
            (0, 0, 0, PartitionFormat::Unknown)
        };

    Some(SourceDiskIdentity {
        disk_guid,
        disk_signature,
        disk_size,
        block_size,
        partition_number,
        partition_start_lba,
        partition_size_blocks,
        partition_format,
    })
}

pub fn source_volume_range(
    base_total_blocks: u64,
    source_disk: Option<SourceDiskIdentity>,
) -> Option<(u64, u64)> {
    let Some(disk) = source_disk.filter(|disk| disk.partition_size_blocks > 0) else {
        return Some((0, base_total_blocks));
    };

    if disk
        .partition_start_lba
        .checked_add(disk.partition_size_blocks)
        .map_or(false, |end| end <= base_total_blocks)
    {
        return Some((disk.partition_start_lba, disk.partition_size_blocks));
    }

    // Some firmware exposes a filesystem partition as its own BlockIO.  That
    // child handle is already partition-relative, while Ventoy-compatible OS
    // parameters still need the parent disk offset stored in SourceDiskIdentity.
    if disk.partition_size_blocks == base_total_blocks {
        return Some((0, base_total_blocks));
    }

    None
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_drive_path() -> Vec<u8> {
        let mut path = Vec::new();
        path.extend_from_slice(&[0x02, 0x01, 0x0c, 0x00, 0xaa, 0xbb, 0, 0, 0, 0, 0, 0]);
        path.extend_from_slice(&[
            DEVICE_PATH_TYPE_MEDIA,
            DEVICE_PATH_SUBTYPE_HARD_DRIVE,
            HARD_DRIVE_DEVICE_PATH_LEN as u8,
            0x00,
        ]);
        path.extend_from_slice(&3u32.to_le_bytes());
        path.extend_from_slice(&2048u64.to_le_bytes());
        path.extend_from_slice(&4096u64.to_le_bytes());
        path.extend_from_slice(&[0x11; 16]);
        path.push(0x02);
        path.push(0x02);
        path.extend_from_slice(&DEVICE_PATH_END_ENTIRE);
        path
    }

    fn source_identity(start_lba: u64, blocks: u64) -> SourceDiskIdentity {
        SourceDiskIdentity {
            disk_guid: [0x42; 16],
            disk_signature: [0xaa, 0xbb, 0xcc, 0xdd],
            disk_size: 0,
            block_size: 512,
            partition_number: 1,
            partition_start_lba: start_lba,
            partition_size_blocks: blocks,
            partition_format: PartitionFormat::Gpt,
        }
    }

    #[test]
    fn parses_hard_drive_node_and_parent_path() {
        let path = hard_drive_path();

        let info = parse_last_hard_drive_device_path(&path).expect("hard drive node");
        let parent = parent_device_path_bytes(&path, &info).expect("parent path");

        assert_eq!(info.node_offset, 12);
        assert_eq!(info.partition_number, 3);
        assert_eq!(info.partition_start_lba, 2048);
        assert_eq!(info.partition_size_blocks, 4096);
        assert_eq!(info.partition_format, PartitionFormat::Gpt);
        assert_eq!(&parent[..12], &path[..12]);
        assert_eq!(&parent[12..], &DEVICE_PATH_END_ENTIRE);
    }

    #[test]
    fn builds_identity_from_ventoy_disk_offsets() {
        let mut first_block = [0u8; 512];
        first_block[VENTOY_DISK_GUID_OFFSET..VENTOY_DISK_GUID_OFFSET + 16]
            .copy_from_slice(&[0x42; 16]);
        first_block[VENTOY_DISK_SIGNATURE_OFFSET..VENTOY_DISK_SIGNATURE_OFFSET + 4]
            .copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let info = HardDriveDevicePathInfo {
            node_offset: 12,
            partition_number: 2,
            partition_start_lba: 4096,
            partition_size_blocks: 8192,
            partition_format: PartitionFormat::Mbr,
            signature_type: 1,
        };

        let identity = build_source_disk_identity(&first_block, 128 * 1024 * 1024, 512, Some(info))
            .expect("identity");

        assert_eq!(identity.disk_guid, [0x42; 16]);
        assert_eq!(identity.disk_signature, [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(identity.partition_number, 2);
        assert_eq!(identity.partition_start_lba, 4096);
        assert_eq!(identity.partition_format, PartitionFormat::Mbr);
    }

    #[test]
    fn source_volume_range_uses_parent_disk_partition_offset() {
        assert_eq!(
            source_volume_range(20_000, Some(source_identity(2048, 4096))),
            Some((2048, 4096))
        );
    }

    #[test]
    fn source_volume_range_accepts_partition_relative_child_block_io() {
        assert_eq!(
            source_volume_range(4096, Some(source_identity(2048, 4096))),
            Some((0, 4096))
        );
    }

    #[test]
    fn source_volume_range_uses_whole_device_without_partition_identity() {
        assert_eq!(source_volume_range(8192, None), Some((0, 8192)));
    }

    #[test]
    fn source_volume_range_rejects_inconsistent_partition_identity() {
        assert_eq!(
            source_volume_range(4095, Some(source_identity(2048, 4096))),
            None
        );
    }
}
