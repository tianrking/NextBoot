use crate::media_grow_util::{
    crc32, device_path_to_vec, div_round_up, read_le_u32, read_le_u64, write_le_u32, write_le_u64,
    zeroed_vec,
};
use crate::source_disk::{parent_device_path_bytes, parse_last_hard_drive_device_path};
use alloc::vec::Vec;
use log::{info, warn};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::BootServices;
use uefi::Handle;

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_HEADER_LBA: u64 = 1;
const GPT_HEADER_SIZE: usize = 92;
const GPT_ENTRY_MAX_BYTES: usize = 1024 * 1024;
const GPT_ENTRY_NAME_OFFSET: usize = 56;
const GPT_ENTRY_NAME_LEN: usize = 72;
const GPT_ENTRY_START_LBA_OFFSET: usize = 32;
const GPT_ENTRY_END_LBA_OFFSET: usize = 40;
const GPT_ENTRY_TYPE_GUID_LEN: usize = 16;
const NEXBOOT_EFI: &str = "NEXBOOT_EFI";
const NEXBOOT_DATA: &str = "NEXBOOT_DATA";
const EXFAT_BOOT_BACKUP_LBA: u64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrowOutcome {
    NotReleaseMedia,
    AlreadyGrown,
    Grown {
        old_blocks: u64,
        new_blocks: u64,
        cluster_count: u32,
    },
}

#[derive(Clone, Copy)]
struct GptInfo {
    old_backup_lba: u64,
    last_usable_lba: u64,
    entry_lba: u64,
    entry_count: usize,
    entry_size: usize,
}

#[derive(Clone, Copy)]
struct PartitionInfo {
    index: usize,
    start_lba: u64,
    end_lba: u64,
}

#[derive(Clone, Copy)]
struct ExfatGrowth {
    old_blocks: u64,
    new_blocks: u64,
    cluster_count: u32,
}

pub(crate) fn grow_boot_media(image: Handle, bt: &BootServices) {
    match try_grow_boot_media(image, bt) {
        Ok(GrowOutcome::Grown {
            old_blocks,
            new_blocks,
            cluster_count,
        }) => info!(
            "Expanded NEXTDATA from {} to {} blocks ({} exFAT clusters)",
            old_blocks, new_blocks, cluster_count
        ),
        Ok(GrowOutcome::AlreadyGrown) => info!("NEXTDATA already matches this media size"),
        Ok(GrowOutcome::NotReleaseMedia) => {}
        Err(reason) => warn!("NEXTDATA auto-grow skipped: {}", reason),
    }
}

fn try_grow_boot_media(image: Handle, bt: &BootServices) -> Result<GrowOutcome, &'static str> {
    let parent_handle = boot_parent_block_handle(image, bt)?;
    let mut block_io = bt
        .open_protocol_exclusive::<BlockIO>(parent_handle)
        .map_err(|_| "could not open parent BlockIO")?;
    let media = block_io.media();
    if !media.is_media_present() || media.is_logical_partition() || media.is_read_only() {
        return Ok(GrowOutcome::NotReleaseMedia);
    }
    let block_size = media.block_size();
    if block_size != 512 && block_size != 4096 {
        return Ok(GrowOutcome::NotReleaseMedia);
    }
    let media_id = media.media_id();
    let total_blocks = media
        .last_block()
        .checked_add(1)
        .ok_or("invalid media block count")?;
    let last_lba = total_blocks.checked_sub(1).ok_or("empty media")?;

    let mut mbr = read_blocks(&block_io, media_id, 0, block_size, 1)?;
    if mbr.get(510..512) != Some(&[0x55, 0xaa]) || mbr.get(0x1be + 4) != Some(&0xee) {
        return Ok(GrowOutcome::NotReleaseMedia);
    }

    let mut header = read_blocks(&block_io, media_id, GPT_HEADER_LBA, block_size, 1)?;
    let gpt = parse_gpt_header(&header, block_size)?;
    let entry_array_blocks = div_round_up(
        gpt.entry_count
            .checked_mul(gpt.entry_size)
            .ok_or("GPT entry array is too large")?,
        block_size as usize,
    );
    if entry_array_blocks == 0 {
        return Ok(GrowOutcome::NotReleaseMedia);
    }
    let entry_array_sectors =
        u64::try_from(entry_array_blocks).map_err(|_| "GPT entry array is too large")?;
    let mut entries = read_blocks(
        &block_io,
        media_id,
        gpt.entry_lba,
        block_size,
        entry_array_sectors,
    )?;
    let entry_bytes = gpt.entry_count * gpt.entry_size;
    let entry_crc = read_le_u32(&header, 88).ok_or("missing GPT entry CRC")?;
    if crc32(entries.get(..entry_bytes).ok_or("truncated GPT entries")?) != entry_crc {
        return Err("GPT entry CRC mismatch");
    }

    let data = find_release_data_partition(&entries, gpt.entry_count, gpt.entry_size)?;
    let backup_entries_lba = last_lba
        .checked_sub(entry_array_sectors)
        .ok_or("media is too small for backup GPT")?;
    let new_last_usable_lba = backup_entries_lba
        .checked_sub(1)
        .ok_or("media is too small for usable GPT range")?;
    if new_last_usable_lba <= data.start_lba {
        return Ok(GrowOutcome::NotReleaseMedia);
    }

    let available_blocks = new_last_usable_lba - data.start_lba + 1;
    let growth = plan_exfat_growth(&block_io, media_id, block_size, data, available_blocks)?;
    let new_data_end = data
        .start_lba
        .checked_add(growth.new_blocks)
        .and_then(|value| value.checked_sub(1))
        .ok_or("expanded partition overflows")?;
    if new_data_end < data.end_lba {
        return Err("existing NEXTDATA is larger than growable exFAT capacity");
    }

    let needs_gpt = gpt.old_backup_lba != last_lba
        || gpt.last_usable_lba != new_last_usable_lba
        || data.end_lba != new_data_end;
    let needs_exfat = growth.old_blocks != growth.new_blocks;
    if !needs_gpt && !needs_exfat {
        return Ok(GrowOutcome::AlreadyGrown);
    }

    if needs_gpt {
        update_gpt_entries(&mut entries, gpt.entry_size, data.index, new_data_end)?;
        let new_entry_crc = crc32(entries.get(..entry_bytes).ok_or("truncated GPT entries")?);
        write_le_u32(
            &mut mbr,
            0x1be + 12,
            (total_blocks - 1).min(u32::MAX as u64) as u32,
        )?;
        block_io
            .write_blocks(media_id, 0, &mbr)
            .map_err(|_| "failed to update protective MBR")?;
        block_io
            .write_blocks(media_id, gpt.entry_lba, &entries)
            .map_err(|_| "failed to update primary GPT entries")?;
        block_io
            .write_blocks(media_id, backup_entries_lba, &entries)
            .map_err(|_| "failed to write backup GPT entries")?;
        update_primary_header(&mut header, last_lba, new_last_usable_lba, new_entry_crc)?;
        block_io
            .write_blocks(media_id, GPT_HEADER_LBA, &header)
            .map_err(|_| "failed to write primary GPT header")?;
        let mut backup_header = header.clone();
        update_backup_header(&mut backup_header, last_lba, backup_entries_lba)?;
        block_io
            .write_blocks(media_id, last_lba, &backup_header)
            .map_err(|_| "failed to write backup GPT header")?;
    }

    if needs_exfat {
        update_exfat_boot(&mut block_io, media_id, block_size, data, growth)?;
    }
    block_io
        .flush_blocks()
        .map_err(|_| "failed to flush media writes")?;
    Ok(GrowOutcome::Grown {
        old_blocks: growth.old_blocks,
        new_blocks: growth.new_blocks,
        cluster_count: growth.cluster_count,
    })
}

fn boot_parent_block_handle(image: Handle, bt: &BootServices) -> Result<Handle, &'static str> {
    let loaded = bt
        .open_protocol_exclusive::<LoadedImage>(image)
        .map_err(|_| "could not open LoadedImage")?;
    let boot_device = loaded.device().ok_or("LoadedImage has no boot device")?;
    drop(loaded);

    let device_path = bt
        .open_protocol_exclusive::<DevicePath>(boot_device)
        .map_err(|_| "could not open boot DevicePath")?;
    let device_path_bytes = device_path_to_vec(&device_path)?;
    drop(device_path);

    let hard_drive = parse_last_hard_drive_device_path(&device_path_bytes)
        .ok_or("boot device has no hard-drive device path")?;
    let parent_path = parent_device_path_bytes(&device_path_bytes, &hard_drive)
        .ok_or("could not derive parent device path")?;
    let mut parent =
        unsafe { DevicePath::from_ffi_ptr(parent_path.as_ptr().cast::<FfiDevicePath>()) };
    bt.locate_device_path::<BlockIO>(&mut parent)
        .map_err(|_| "could not locate parent BlockIO")
}

fn parse_gpt_header(header: &[u8], block_size: u32) -> Result<GptInfo, &'static str> {
    if header.get(0..8) != Some(&GPT_SIGNATURE[..]) {
        return Err("missing GPT header");
    }
    let header_size = read_le_u32(header, 12).ok_or("missing GPT header size")? as usize;
    if !(GPT_HEADER_SIZE..=block_size as usize).contains(&header_size) {
        return Err("unsupported GPT header size");
    }
    let expected_crc = read_le_u32(header, 16).ok_or("missing GPT header CRC")?;
    let mut scratch = zeroed_vec(header_size);
    scratch.copy_from_slice(header.get(..header_size).ok_or("truncated GPT header")?);
    write_le_u32(&mut scratch, 16, 0)?;
    if crc32(&scratch) != expected_crc {
        return Err("GPT header CRC mismatch");
    }
    if read_le_u64(header, 24).ok_or("missing GPT current LBA")? != GPT_HEADER_LBA {
        return Err("GPT primary header is not at LBA 1");
    }

    let entry_count = read_le_u32(header, 80).ok_or("missing GPT entry count")? as usize;
    let entry_size = read_le_u32(header, 84).ok_or("missing GPT entry size")? as usize;
    if entry_count == 0 || entry_size < 128 || entry_size % 8 != 0 {
        return Err("unsupported GPT entry geometry");
    }
    if entry_count
        .checked_mul(entry_size)
        .map_or(true, |bytes| bytes > GPT_ENTRY_MAX_BYTES)
    {
        return Err("GPT entry array is too large");
    }
    Ok(GptInfo {
        old_backup_lba: read_le_u64(header, 32).ok_or("missing GPT backup LBA")?,
        last_usable_lba: read_le_u64(header, 48).ok_or("missing GPT last usable LBA")?,
        entry_lba: read_le_u64(header, 72).ok_or("missing GPT entry LBA")?,
        entry_count,
        entry_size,
    })
}

fn find_release_data_partition(
    entries: &[u8],
    entry_count: usize,
    entry_size: usize,
) -> Result<PartitionInfo, &'static str> {
    let mut saw_efi = false;
    let mut data = None;
    for index in 0..entry_count {
        let offset = index
            .checked_mul(entry_size)
            .ok_or("GPT entry offset overflow")?;
        let entry = entries
            .get(offset..offset + entry_size)
            .ok_or("truncated GPT entry")?;
        if entry
            .get(..GPT_ENTRY_TYPE_GUID_LEN)
            .is_some_and(|guid| guid.iter().all(|byte| *byte == 0))
        {
            continue;
        }
        let info = PartitionInfo {
            index,
            start_lba: read_le_u64(entry, GPT_ENTRY_START_LBA_OFFSET)
                .ok_or("missing partition start")?,
            end_lba: read_le_u64(entry, GPT_ENTRY_END_LBA_OFFSET).ok_or("missing partition end")?,
        };
        if gpt_name_eq(entry, NEXBOOT_EFI) {
            saw_efi = true;
            continue;
        }
        if gpt_name_eq(entry, NEXBOOT_DATA) {
            data = Some(info);
        }
    }
    if !saw_efi {
        return Err("missing NEXBOOT_EFI partition");
    }
    let data = data.ok_or("missing NEXBOOT_DATA partition")?;

    for index in 0..entry_count {
        let offset = index
            .checked_mul(entry_size)
            .ok_or("GPT entry offset overflow")?;
        let entry = entries
            .get(offset..offset + entry_size)
            .ok_or("truncated GPT entry")?;
        if entry
            .get(..GPT_ENTRY_TYPE_GUID_LEN)
            .is_some_and(|guid| guid.iter().all(|byte| *byte == 0))
        {
            continue;
        }
        let start_lba =
            read_le_u64(entry, GPT_ENTRY_START_LBA_OFFSET).ok_or("missing partition start")?;
        if start_lba > data.start_lba {
            return Err("non-empty partition follows NEXBOOT_DATA");
        }
    }

    Ok(data)
}

fn plan_exfat_growth(
    block_io: &BlockIO,
    media_id: u32,
    block_size: u32,
    data: PartitionInfo,
    available_blocks: u64,
) -> Result<ExfatGrowth, &'static str> {
    let boot = read_blocks(block_io, media_id, data.start_lba, block_size, 1)?;
    if boot.get(0..3) != Some(&[0xeb, 0x76, 0x90]) || boot.get(3..11) != Some(&b"EXFAT   "[..]) {
        return Err("NEXTDATA is not exFAT");
    }
    if read_le_u64(&boot, 64).ok_or("missing exFAT partition offset")? != data.start_lba {
        return Err("exFAT partition offset does not match GPT");
    }
    let bytes_per_sector = 1u64
        .checked_shl(*boot.get(108).ok_or("missing exFAT sector shift")? as u32)
        .ok_or("invalid exFAT sector shift")?;
    if bytes_per_sector != u64::from(block_size) {
        return Err("exFAT sector size does not match media");
    }
    let sectors_per_cluster = 1u64
        .checked_shl(*boot.get(109).ok_or("missing exFAT cluster shift")? as u32)
        .ok_or("invalid exFAT cluster shift")?;
    let fat_length = u64::from(read_le_u32(&boot, 84).ok_or("missing exFAT FAT length")?);
    let cluster_heap_offset =
        u64::from(read_le_u32(&boot, 88).ok_or("missing exFAT cluster heap offset")?);
    if fat_length == 0 || cluster_heap_offset <= 24 || available_blocks <= cluster_heap_offset {
        return Err("invalid growable exFAT geometry");
    }
    let fat_capacity = fat_length
        .checked_mul(u64::from(block_size))
        .and_then(|bytes| bytes.checked_div(4))
        .and_then(|entries| entries.checked_sub(2))
        .ok_or("invalid exFAT FAT capacity")?;
    let requested_clusters = (available_blocks - cluster_heap_offset) / sectors_per_cluster;
    let cluster_count = requested_clusters
        .min(fat_capacity)
        .min(u64::from(u32::MAX - 2));
    if cluster_count < 16 {
        return Err("expanded exFAT cluster count is too small");
    }
    let new_blocks = cluster_heap_offset
        .checked_add(
            cluster_count
                .checked_mul(sectors_per_cluster)
                .ok_or("exFAT growth overflows")?,
        )
        .ok_or("exFAT growth overflows")?;
    Ok(ExfatGrowth {
        old_blocks: read_le_u64(&boot, 72).ok_or("missing exFAT volume length")?,
        new_blocks,
        cluster_count: u32::try_from(cluster_count).map_err(|_| "exFAT cluster count overflows")?,
    })
}

fn update_exfat_boot(
    block_io: &mut BlockIO,
    media_id: u32,
    block_size: u32,
    data: PartitionInfo,
    growth: ExfatGrowth,
) -> Result<(), &'static str> {
    let mut boot = read_blocks(block_io, media_id, data.start_lba, block_size, 1)?;
    write_le_u64(&mut boot, 72, growth.new_blocks)?;
    write_le_u32(&mut boot, 92, growth.cluster_count)?;
    block_io
        .write_blocks(media_id, data.start_lba, &boot)
        .map_err(|_| "failed to write exFAT boot sector")?;
    block_io
        .write_blocks(media_id, data.start_lba + EXFAT_BOOT_BACKUP_LBA, &boot)
        .map_err(|_| "failed to write exFAT backup boot sector")
}

fn update_gpt_entries(
    entries: &mut [u8],
    entry_size: usize,
    index: usize,
    new_end_lba: u64,
) -> Result<(), &'static str> {
    let offset = index
        .checked_mul(entry_size)
        .and_then(|value| value.checked_add(GPT_ENTRY_END_LBA_OFFSET))
        .ok_or("GPT entry offset overflows")?;
    write_le_u64(entries, offset, new_end_lba)
}

fn update_primary_header(
    header: &mut [u8],
    backup_lba: u64,
    last_usable_lba: u64,
    entry_crc: u32,
) -> Result<(), &'static str> {
    write_le_u64(header, 32, backup_lba)?;
    write_le_u64(header, 48, last_usable_lba)?;
    write_le_u32(header, 88, entry_crc)?;
    update_header_crc(header)
}

fn update_backup_header(
    header: &mut [u8],
    current_lba: u64,
    entries_lba: u64,
) -> Result<(), &'static str> {
    write_le_u64(header, 24, current_lba)?;
    write_le_u64(header, 32, GPT_HEADER_LBA)?;
    write_le_u64(header, 72, entries_lba)?;
    update_header_crc(header)
}

fn update_header_crc(header: &mut [u8]) -> Result<(), &'static str> {
    let header_size = read_le_u32(header, 12).ok_or("missing GPT header size")? as usize;
    if header_size > header.len() {
        return Err("truncated GPT header");
    }
    write_le_u32(header, 16, 0)?;
    let crc = crc32(header.get(..header_size).ok_or("truncated GPT header")?);
    write_le_u32(header, 16, crc)
}

fn read_blocks(
    block_io: &BlockIO,
    media_id: u32,
    lba: u64,
    block_size: u32,
    block_count: u64,
) -> Result<Vec<u8>, &'static str> {
    let len = usize::try_from(
        block_count
            .checked_mul(u64::from(block_size))
            .ok_or("block read size overflows")?,
    )
    .map_err(|_| "block read size is too large")?;
    let mut data = zeroed_vec(len);
    block_io
        .read_blocks(media_id, lba, &mut data)
        .map_err(|_| "failed to read media blocks")?;
    Ok(data)
}

fn gpt_name_eq(entry: &[u8], expected: &str) -> bool {
    let Some(raw) = entry.get(GPT_ENTRY_NAME_OFFSET..GPT_ENTRY_NAME_OFFSET + GPT_ENTRY_NAME_LEN)
    else {
        return false;
    };
    let mut offset = 0usize;
    for byte in expected.bytes() {
        if raw.get(offset) != Some(&byte) || raw.get(offset + 1) != Some(&0) {
            return false;
        }
        offset += 2;
    }
    raw.get(offset..)
        .is_some_and(|tail| tail.iter().all(|byte| *byte == 0))
}
