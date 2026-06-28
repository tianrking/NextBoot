use super::*;
use alloc::vec;
use alloc::vec::Vec;

#[test]
fn parses_4k_gpt_split_layout() {
    let disk = make_gpt_disk(4096, 8, 128, 4);

    assert_eq!(detect_block_size(&disk), 4096);

    let parsed = GptDisk::parse(&disk, 4096).expect("4K GPT");
    assert_eq!(parsed.block_size, 4096);
    assert_eq!(parsed.partitions.len(), 2);

    let esp = parsed.get_esp_partition().expect("esp");
    assert_eq!(esp.name, "NEXBOOT_EFI");
    assert_eq!(esp.start_lba, 3);
    assert_eq!(esp.size_bytes(4096), 4096);

    let data = parsed.get_data_partition().expect("data");
    assert_eq!(data.name, "NEXBOOT_DATA");
    assert!(data.is_microsoft_basic());
}

#[test]
fn rejects_too_small_partition_entry_size() {
    let disk = make_gpt_disk(512, 16, 64, 4);

    assert!(matches!(
        GptDisk::parse(&disk, 512),
        Err(FsError::InvalidSignature)
    ));
}

#[test]
fn rejects_truncated_partition_entry_array() {
    let mut disk = make_gpt_disk(512, 16, 128, 4);
    disk.truncate(2 * 512 + 128);

    assert!(matches!(
        GptDisk::parse(&disk, 512),
        Err(FsError::ReadError)
    ));
}

#[test]
fn rejects_partition_outside_usable_lba_range() {
    let mut disk = make_gpt_disk(512, 16, 128, 4);
    let first_entry = 2 * 512;
    write_le_u64(&mut disk, first_entry + 40, 15);

    assert!(matches!(
        GptDisk::parse(&disk, 512),
        Err(FsError::Corrupted)
    ));
}

#[test]
fn detects_4k_signature_at_exact_slice_end() {
    let mut disk = vec![0u8; 4096 + 8];
    write_protective_mbr(&mut disk, 16);
    disk[4096..4096 + 8].copy_from_slice(b"EFI PART");

    assert_eq!(detect_block_size(&disk), 4096);
}

fn make_gpt_disk(
    block_size: usize,
    block_count: u64,
    entry_size: usize,
    entry_count: usize,
) -> Vec<u8> {
    let mut disk = vec![0u8; block_size * block_count as usize];
    write_protective_mbr(&mut disk, block_count);

    let header = block_size;
    disk[header..header + 8].copy_from_slice(b"EFI PART");
    write_le_u32(&mut disk, header + 8, 0x0001_0000);
    write_le_u32(&mut disk, header + 12, 92);
    write_le_u64(&mut disk, header + 24, 1);
    write_le_u64(&mut disk, header + 32, block_count - 1);
    write_le_u64(&mut disk, header + 40, 3);
    write_le_u64(&mut disk, header + 48, block_count - 2);
    write_le_u64(&mut disk, header + 72, 2);
    write_le_u32(&mut disk, header + 80, entry_count as u32);
    write_le_u32(&mut disk, header + 84, entry_size as u32);

    if entry_size >= 128 {
        write_partition(
            &mut disk,
            2 * block_size,
            partition_types::ESP,
            3,
            3,
            "NEXBOOT_EFI",
        );
        write_partition(
            &mut disk,
            2 * block_size + entry_size,
            partition_types::MICROSOFT_BASIC,
            4,
            block_count - 2,
            "NEXBOOT_DATA",
        );
    }

    disk
}

fn write_protective_mbr(disk: &mut [u8], block_count: u64) {
    disk[0x1be + 4] = 0xee;
    disk[0x1be + 8..0x1be + 12].copy_from_slice(&1u32.to_le_bytes());
    let sectors = block_count.min(u64::from(u32::MAX)) as u32;
    disk[0x1be + 12..0x1be + 16].copy_from_slice(&sectors.to_le_bytes());
    disk[510] = 0x55;
    disk[511] = 0xaa;
}

fn write_partition(
    disk: &mut [u8],
    offset: usize,
    type_guid: [u8; 16],
    start_lba: u64,
    end_lba: u64,
    name: &str,
) {
    disk[offset..offset + 16].copy_from_slice(&type_guid);
    disk[offset + 16..offset + 32].copy_from_slice(&[0x42; 16]);
    write_le_u64(disk, offset + 32, start_lba);
    write_le_u64(disk, offset + 40, end_lba);

    for (index, code_unit) in name.encode_utf16().take(36).enumerate() {
        let offset = offset + 56 + index * 2;
        disk[offset..offset + 2].copy_from_slice(&code_unit.to_le_bytes());
    }
}

fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
