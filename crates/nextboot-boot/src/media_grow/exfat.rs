use super::{read_blocks, PartitionInfo};
use crate::media_grow_util::{read_le_u32, read_le_u64, write_le_u64};
use uefi::proto::media::block::BlockIO;

const EXFAT_BOOT_REGION_SECTORS: u64 = 12;
const EXFAT_FAT_OFFSET: usize = 80;
const EXFAT_FAT_LENGTH: usize = 84;
const EXFAT_CLUSTER_HEAP_OFFSET: usize = 88;
const EXFAT_CLUSTER_COUNT: usize = 92;
const EXFAT_ROOT_CLUSTER: usize = 96;
const EXFAT_BYTES_PER_SECTOR_SHIFT: usize = 108;
const EXFAT_SECTORS_PER_CLUSTER_SHIFT: usize = 109;
const EXFAT_ALLOCATION_BITMAP_ENTRY: u8 = 0x81;
const EXFAT_CLUSTER_EOC: u32 = 0xffff_ffff;

pub(super) fn update_allocation_bitmap_length(
    block_io: &mut BlockIO,
    media_id: u32,
    block_size: u32,
    data: PartitionInfo,
    boot: &[u8],
    new_cluster_count: u32,
) -> Result<(), &'static str> {
    let bytes_per_sector = 1u64
        .checked_shl(
            *boot
                .get(EXFAT_BYTES_PER_SECTOR_SHIFT)
                .ok_or("missing exFAT sector shift")? as u32,
        )
        .ok_or("invalid exFAT sector shift")?;
    if bytes_per_sector != u64::from(block_size) {
        return Err("exFAT sector size does not match media");
    }
    let sectors_per_cluster = 1u64
        .checked_shl(
            *boot
                .get(EXFAT_SECTORS_PER_CLUSTER_SHIFT)
                .ok_or("missing exFAT cluster shift")? as u32,
        )
        .ok_or("invalid exFAT cluster shift")?;
    let fat_offset =
        u64::from(read_le_u32(boot, EXFAT_FAT_OFFSET).ok_or("missing exFAT FAT offset")?);
    let fat_length =
        u64::from(read_le_u32(boot, EXFAT_FAT_LENGTH).ok_or("missing exFAT FAT length")?);
    let cluster_heap_offset = u64::from(
        read_le_u32(boot, EXFAT_CLUSTER_HEAP_OFFSET).ok_or("missing exFAT cluster heap offset")?,
    );
    let old_cluster_count =
        read_le_u32(boot, EXFAT_CLUSTER_COUNT).ok_or("missing exFAT cluster count")?;
    let root_cluster = read_le_u32(boot, EXFAT_ROOT_CLUSTER).ok_or("missing exFAT root cluster")?;
    if fat_length == 0
        || fat_offset < EXFAT_BOOT_REGION_SECTORS * 2
        || fat_offset
            .checked_add(fat_length)
            .ok_or("exFAT FAT overflows")?
            > cluster_heap_offset
        || root_cluster < 2
        || root_cluster > old_cluster_count.saturating_add(1)
    {
        return Err("invalid exFAT bitmap growth geometry");
    }

    let cluster_bytes = sectors_per_cluster
        .checked_mul(u64::from(block_size))
        .ok_or("exFAT cluster size overflows")?;
    let required_bitmap_bytes = (u64::from(new_cluster_count) + 7) / 8;
    let required_bitmap_clusters = required_bitmap_bytes
        .checked_add(cluster_bytes - 1)
        .ok_or("exFAT bitmap size overflows")?
        / cluster_bytes;

    let mut current = root_cluster;
    let mut root_steps = 0u32;
    loop {
        root_steps = root_steps
            .checked_add(1)
            .ok_or("exFAT root chain overflows")?;
        if root_steps > old_cluster_count {
            return Err("invalid exFAT root directory chain");
        }
        let cluster_lba = data
            .start_lba
            .checked_add(cluster_heap_offset)
            .and_then(|value| {
                value.checked_add(u64::from(current - 2).checked_mul(sectors_per_cluster)?)
            })
            .ok_or("exFAT root cluster offset overflows")?;
        let mut directory = read_blocks(
            block_io,
            media_id,
            cluster_lba,
            block_size,
            sectors_per_cluster,
        )?;
        for offset in (0..directory.len()).step_by(32) {
            let entry_type = *directory.get(offset).ok_or("truncated exFAT directory")?;
            if entry_type == 0 {
                break;
            }
            if entry_type != EXFAT_ALLOCATION_BITMAP_ENTRY {
                continue;
            }
            let bitmap_cluster =
                read_le_u32(&directory, offset + 20).ok_or("truncated exFAT allocation bitmap")?;
            let bitmap_length =
                read_le_u64(&directory, offset + 24).ok_or("truncated exFAT allocation bitmap")?;
            let bitmap_clusters = exfat_chain_length(
                block_io,
                media_id,
                block_size,
                data,
                fat_offset,
                bitmap_cluster,
                old_cluster_count,
            )?;
            let expected_old_clusters = bitmap_length
                .checked_add(cluster_bytes - 1)
                .ok_or("exFAT bitmap length overflows")?
                / cluster_bytes;
            if bitmap_clusters != expected_old_clusters
                || bitmap_clusters != required_bitmap_clusters
            {
                return Err("exFAT allocation bitmap chain cannot cover expanded volume");
            }
            write_le_u64(&mut directory, offset + 24, required_bitmap_bytes)?;
            block_io
                .write_blocks(media_id, cluster_lba, &directory)
                .map_err(|_| "failed to update exFAT allocation bitmap")?;
            return Ok(());
        }
        let next = exfat_fat_entry(block_io, media_id, block_size, data, fat_offset, current)?;
        if next == EXFAT_CLUSTER_EOC {
            break;
        }
        if next < 2 || next > old_cluster_count.saturating_add(1) {
            return Err("invalid exFAT root directory chain");
        }
        current = next;
    }
    Err("missing exFAT allocation bitmap")
}

fn exfat_chain_length(
    block_io: &BlockIO,
    media_id: u32,
    block_size: u32,
    data: PartitionInfo,
    fat_offset: u64,
    first_cluster: u32,
    cluster_count: u32,
) -> Result<u64, &'static str> {
    if first_cluster < 2 || first_cluster > cluster_count.saturating_add(1) {
        return Err("invalid exFAT allocation bitmap cluster");
    }
    let mut current = first_cluster;
    let mut length = 0u64;
    loop {
        length = length
            .checked_add(1)
            .ok_or("exFAT bitmap chain overflows")?;
        if length > u64::from(cluster_count) {
            return Err("invalid exFAT allocation bitmap chain");
        }
        let next = exfat_fat_entry(block_io, media_id, block_size, data, fat_offset, current)?;
        if next == EXFAT_CLUSTER_EOC {
            return Ok(length);
        }
        if next < 2 || next > cluster_count.saturating_add(1) || next <= current {
            return Err("invalid exFAT allocation bitmap chain");
        }
        current = next;
    }
}

fn exfat_fat_entry(
    block_io: &BlockIO,
    media_id: u32,
    block_size: u32,
    data: PartitionInfo,
    fat_offset: u64,
    cluster: u32,
) -> Result<u32, &'static str> {
    let entry_offset = u64::from(cluster)
        .checked_mul(4)
        .ok_or("exFAT FAT entry offset overflows")?;
    let sector_offset = entry_offset / u64::from(block_size);
    let offset_in_sector = usize::try_from(entry_offset % u64::from(block_size))
        .map_err(|_| "exFAT FAT entry offset is invalid")?;
    let sector = read_blocks(
        block_io,
        media_id,
        data.start_lba
            .checked_add(fat_offset)
            .and_then(|value| value.checked_add(sector_offset))
            .ok_or("exFAT FAT sector offset overflows")?,
        block_size,
        1,
    )?;
    read_le_u32(&sector, offset_in_sector).ok_or("truncated exFAT FAT entry")
}
