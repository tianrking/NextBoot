use super::model::{MbrPartitionEntry, PartitionCandidate, PartitionRange};
use super::{
    alloc_buffer_for_block, has_mbr_signature, read_block_range, read_le_u32, read_le_u64,
};
use crate::source_disk::PartitionFormat;
use alloc::vec::Vec;
use nextboot_fs::BlockIoOps;

const MBR_PARTITION_TABLE_OFFSET: usize = 0x1be;
const MBR_PARTITION_ENTRY_SIZE: usize = 16;
const MBR_PRIMARY_PARTITION_COUNT: usize = 4;
const MBR_LOGICAL_PARTITION_NUMBER_BASE: u32 = 5;
const MBR_MAX_LOGICAL_PARTITIONS: usize = 128;
const GPT_HEADER_LBA: u64 = 1;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_HEADER_MIN_SIZE: u32 = 92;
const GPT_PARTITION_ENTRY_LBA_OFFSET: usize = 72;
const GPT_NUM_PARTITION_ENTRIES_OFFSET: usize = 80;
const GPT_PARTITION_ENTRY_SIZE_OFFSET: usize = 84;
const GPT_MIN_PARTITION_ENTRY_SIZE: usize = 128;
const GPT_MAX_PARTITION_ENTRY_SIZE: usize = 4096;
const GPT_MAX_PARTITION_ENTRIES: usize = 4096;
const GPT_MAX_PARTITION_ENTRY_ARRAY_BYTES: usize = 1024 * 1024;

pub(super) fn discover_partition_candidates(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Vec<PartitionCandidate> {
    if let Some(partitions) = discover_gpt_partitions(shared.clone(), first_block) {
        return partitions;
    }
    discover_mbr_partitions(shared, first_block)
}

fn discover_gpt_partitions(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Option<Vec<PartitionCandidate>> {
    if !has_mbr_signature(first_block) {
        return None;
    }
    let has_protective = (0..4).any(|index| {
        first_block
            .get(MBR_PARTITION_TABLE_OFFSET + index * MBR_PARTITION_ENTRY_SIZE + 4)
            .copied()
            .unwrap_or(0)
            == 0xee
    });
    if !has_protective {
        return None;
    }

    let header_block = read_one_block(&shared, GPT_HEADER_LBA)?;
    let header = header_block.as_slice();
    if header.get(0..8)? != GPT_SIGNATURE {
        return None;
    }

    let header_size = read_le_u32(header, 12)?;
    if header_size < GPT_HEADER_MIN_SIZE
        || usize::try_from(header_size)
            .ok()
            .map_or(true, |len| len > header.len())
    {
        return None;
    }
    let entry_lba = read_le_u64(header, GPT_PARTITION_ENTRY_LBA_OFFSET)?;
    let num_entries = read_le_u32(header, GPT_NUM_PARTITION_ENTRIES_OFFSET)?;
    let entry_size = read_le_u32(header, GPT_PARTITION_ENTRY_SIZE_OFFSET)?;
    let entry_size = usize::try_from(entry_size).ok()?;
    if !(GPT_MIN_PARTITION_ENTRY_SIZE..=GPT_MAX_PARTITION_ENTRY_SIZE).contains(&entry_size) {
        return None;
    }

    let num_entries = usize::try_from(num_entries)
        .ok()?
        .min(GPT_MAX_PARTITION_ENTRIES);
    let entry_bytes_len = num_entries.checked_mul(entry_size)?;
    if entry_bytes_len == 0 || entry_bytes_len > GPT_MAX_PARTITION_ENTRY_ARRAY_BYTES {
        return None;
    }
    let entry_bytes = read_block_range(&shared, entry_lba, entry_bytes_len)?;

    let mut out = Vec::new();
    for index in 0..num_entries {
        let offset = index.checked_mul(entry_size)?;
        let entry = match entry_bytes.get(offset..offset + entry_size) {
            Some(entry) => entry,
            None => break,
        };
        if entry.get(0..16)?.iter().all(|byte| *byte == 0) {
            continue;
        }
        let start_lba = read_le_u64(entry, 32)?;
        let end_lba = read_le_u64(entry, 40)?;
        if start_lba == 0 || end_lba < start_lba {
            continue;
        }
        if out.try_reserve_exact(1).is_err() {
            break;
        }
        out.push(PartitionCandidate {
            number: u32::try_from(index + 1).ok()?,
            start_lba,
            block_count: end_lba - start_lba + 1,
            format: PartitionFormat::Gpt,
        });
    }

    Some(out)
}

fn discover_mbr_partitions(
    shared: nextboot_fs::SharedBlockIo,
    first_block: &[u8],
) -> Vec<PartitionCandidate> {
    let mut out = Vec::new();
    if !has_mbr_signature(first_block) {
        return out;
    }

    let mut extended_ranges = Vec::new();
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(first_block, index) else {
            continue;
        };
        if partition.partition_type == 0xee
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }

        if is_extended_mbr_partition(partition.partition_type) {
            if extended_ranges.try_reserve_exact(1).is_err() {
                continue;
            }
            extended_ranges.push(PartitionRange {
                start_lba: u64::from(partition.start_lba),
                block_count: u64::from(partition.total_sectors),
            });
            continue;
        }

        if !push_mbr_partition_candidate(
            &mut out,
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            u64::from(partition.start_lba),
            u64::from(partition.total_sectors),
        ) {
            break;
        }
    }

    let mut logical_number = MBR_LOGICAL_PARTITION_NUMBER_BASE;
    for extended in extended_ranges {
        discover_mbr_logical_partitions(&shared, extended, &mut out, &mut logical_number);
    }

    out
}

fn discover_mbr_logical_partitions(
    shared: &nextboot_fs::SharedBlockIo,
    extended: PartitionRange,
    out: &mut Vec<PartitionCandidate>,
    logical_number: &mut u32,
) {
    if extended.block_count == 0 || !range_contains_lba(extended, extended.start_lba) {
        return;
    }

    let mut visited = Vec::new();
    let mut current_ebr_lba = extended.start_lba;

    for _ in 0..MBR_MAX_LOGICAL_PARTITIONS {
        if current_ebr_lba >= shared.total_blocks()
            || !range_contains_lba(extended, current_ebr_lba)
            || visited.iter().any(|lba| *lba == current_ebr_lba)
        {
            break;
        }
        if visited.try_reserve_exact(1).is_err() {
            break;
        }
        visited.push(current_ebr_lba);

        let Some(ebr) = read_one_block(shared, current_ebr_lba) else {
            break;
        };
        if !has_mbr_signature(&ebr) {
            break;
        }

        if let Some(logical) = find_logical_mbr_partition(&ebr) {
            if let Some(start_lba) = current_ebr_lba.checked_add(u64::from(logical.start_lba)) {
                let block_count = u64::from(logical.total_sectors);
                if range_contains_extent(extended, start_lba, block_count)
                    && range_fits_disk(start_lba, block_count, shared.total_blocks())
                {
                    if !push_mbr_partition_candidate(out, *logical_number, start_lba, block_count) {
                        return;
                    }
                    *logical_number = (*logical_number).saturating_add(1);
                }
            }
        }

        let Some(next_ebr_lba) =
            find_next_ebr_lba(&ebr, extended, current_ebr_lba, shared.total_blocks())
        else {
            break;
        };
        current_ebr_lba = next_ebr_lba;
    }
}

fn find_logical_mbr_partition(block: &[u8]) -> Option<MbrPartitionEntry> {
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(block, index) else {
            continue;
        };
        if partition.partition_type == 0xee
            || is_extended_mbr_partition(partition.partition_type)
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }
        return Some(partition);
    }
    None
}

fn find_next_ebr_lba(
    block: &[u8],
    extended: PartitionRange,
    current_ebr_lba: u64,
    total_blocks: u64,
) -> Option<u64> {
    for index in 0..MBR_PRIMARY_PARTITION_COUNT {
        let Some(partition) = parse_mbr_partition(block, index) else {
            continue;
        };
        if !is_extended_mbr_partition(partition.partition_type)
            || partition.start_lba == 0
            || partition.total_sectors == 0
        {
            continue;
        }
        let next_ebr_lba = extended
            .start_lba
            .checked_add(u64::from(partition.start_lba))?;
        if next_ebr_lba == current_ebr_lba
            || next_ebr_lba >= total_blocks
            || !range_contains_lba(extended, next_ebr_lba)
        {
            continue;
        }
        return Some(next_ebr_lba);
    }
    None
}

fn parse_mbr_partition(block: &[u8], index: usize) -> Option<MbrPartitionEntry> {
    if index >= MBR_PRIMARY_PARTITION_COUNT || !has_mbr_signature(block) {
        return None;
    }
    let offset =
        MBR_PARTITION_TABLE_OFFSET.checked_add(index.checked_mul(MBR_PARTITION_ENTRY_SIZE)?)?;
    let partition_type = block.get(offset + 4).copied()?;
    if partition_type == 0 {
        return None;
    }
    Some(MbrPartitionEntry {
        partition_type,
        start_lba: read_le_u32(block, offset + 8)?,
        total_sectors: read_le_u32(block, offset + 12)?,
    })
}

fn push_mbr_partition_candidate(
    out: &mut Vec<PartitionCandidate>,
    number: u32,
    start_lba: u64,
    block_count: u64,
) -> bool {
    if out.try_reserve_exact(1).is_err() {
        return false;
    }
    out.push(PartitionCandidate {
        number,
        start_lba,
        block_count,
        format: PartitionFormat::Mbr,
    });
    true
}

fn read_one_block(shared: &nextboot_fs::SharedBlockIo, lba: u64) -> Option<Vec<u8>> {
    if lba >= shared.total_blocks() {
        return None;
    }
    let mut bytes = alloc_buffer_for_block(shared.block_size()).ok()?;
    shared.read_blocks(lba, &mut bytes).ok()?;
    Some(bytes)
}

fn is_extended_mbr_partition(partition_type: u8) -> bool {
    matches!(partition_type, 0x05 | 0x0f | 0x85)
}

fn range_contains_lba(range: PartitionRange, lba: u64) -> bool {
    range.block_count != 0 && lba >= range.start_lba && lba - range.start_lba < range.block_count
}

fn range_contains_extent(range: PartitionRange, start_lba: u64, block_count: u64) -> bool {
    if block_count == 0 || start_lba < range.start_lba {
        return false;
    }
    let Some(end_lba) = start_lba.checked_add(block_count) else {
        return false;
    };
    let Some(range_end_lba) = range.start_lba.checked_add(range.block_count) else {
        return false;
    };
    end_lba <= range_end_lba
}

fn range_fits_disk(start_lba: u64, block_count: u64, total_blocks: u64) -> bool {
    block_count != 0
        && start_lba
            .checked_add(block_count)
            .is_some_and(|end_lba| end_lba <= total_blocks)
}

#[cfg(test)]
mod tests;
