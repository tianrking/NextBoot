use super::*;
use crate::source_disk::PartitionFormat;
use alloc::rc::Rc;
use alloc::vec::Vec;
use nextboot_fs::{BlockIoOps, FsError};

struct MemoryBlockIo {
    block_size: usize,
    bytes: Vec<u8>,
}

impl MemoryBlockIo {
    fn new(block_count: usize) -> Self {
        Self::with_block_size(block_count, 512)
    }

    fn with_block_size(block_count: usize, block_size: usize) -> Self {
        let mut bytes = Vec::new();
        bytes.resize(block_count * block_size, 0);
        Self { block_size, bytes }
    }

    fn block(&self, lba: usize) -> &[u8] {
        let start = lba * self.block_size;
        &self.bytes[start..start + self.block_size]
    }

    fn block_mut(&mut self, lba: usize) -> &mut [u8] {
        let start = lba * self.block_size;
        &mut self.bytes[start..start + self.block_size]
    }
}

impl BlockIoOps for MemoryBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size as u32
    }

    fn total_blocks(&self) -> u64 {
        (self.bytes.len() / self.block_size) as u64
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        if buf.is_empty() || buf.len() % self.block_size != 0 {
            return Err(FsError::InvalidArgument);
        }
        let start = usize::try_from(lba).map_err(|_| FsError::ReadError)?;
        let block_count = buf.len() / self.block_size;
        let end_block = start.checked_add(block_count).ok_or(FsError::ReadError)?;
        let byte_start = start
            .checked_mul(self.block_size)
            .ok_or(FsError::ReadError)?;
        let byte_end = end_block
            .checked_mul(self.block_size)
            .ok_or(FsError::ReadError)?;
        let bytes = self
            .bytes
            .get(byte_start..byte_end)
            .ok_or(FsError::ReadError)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

fn write_mbr_entry(
    block: &mut [u8],
    index: usize,
    partition_type: u8,
    start_lba: u32,
    total_sectors: u32,
) {
    block[510] = 0x55;
    block[511] = 0xaa;
    let offset = MBR_PARTITION_TABLE_OFFSET + index * MBR_PARTITION_ENTRY_SIZE;
    block[offset + 4] = partition_type;
    block[offset + 8..offset + 12].copy_from_slice(&start_lba.to_le_bytes());
    block[offset + 12..offset + 16].copy_from_slice(&total_sectors.to_le_bytes());
}

fn write_protective_mbr(block: &mut [u8]) {
    write_mbr_entry(block, 0, 0xee, 1, u32::MAX);
}

fn write_gpt_header(block: &mut [u8], entry_lba: u64, num_entries: u32, entry_size: u32) {
    block[0..8].copy_from_slice(GPT_SIGNATURE);
    block[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    block[12..16].copy_from_slice(&GPT_HEADER_MIN_SIZE.to_le_bytes());
    block[24..32].copy_from_slice(&GPT_HEADER_LBA.to_le_bytes());
    block[72..80].copy_from_slice(&entry_lba.to_le_bytes());
    block[80..84].copy_from_slice(&num_entries.to_le_bytes());
    block[84..88].copy_from_slice(&entry_size.to_le_bytes());
}

fn write_gpt_entry(block: &mut [u8], offset: usize, start_lba: u64, end_lba: u64) {
    let entry = &mut block[offset..offset + GPT_MIN_PARTITION_ENTRY_SIZE];
    entry[0] = 1;
    entry[16] = 2;
    entry[32..40].copy_from_slice(&start_lba.to_le_bytes());
    entry[40..48].copy_from_slice(&end_lba.to_le_bytes());
}

#[test]
fn discovers_mbr_logical_partitions_from_ebr_chain() {
    let mut disk = MemoryBlockIo::new(32_000);
    write_mbr_entry(disk.block_mut(0), 1, 0x07, 2048, 4096);
    write_mbr_entry(disk.block_mut(0), 2, 0x0f, 10_000, 10_000);
    write_mbr_entry(disk.block_mut(10_000), 0, 0x07, 63, 1000);
    write_mbr_entry(disk.block_mut(10_000), 1, 0x05, 2000, 8000);
    write_mbr_entry(disk.block_mut(12_000), 0, 0x0b, 128, 500);

    let first_block = disk.block(0).to_vec();
    let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
    let partitions = discover_mbr_partitions(shared, &first_block);

    assert_eq!(partitions.len(), 3);
    assert_eq!(partitions[0].number, 2);
    assert_eq!(partitions[0].start_lba, 2048);
    assert_eq!(partitions[0].block_count, 4096);
    assert_eq!(partitions[0].format, PartitionFormat::Mbr);
    assert_eq!(partitions[1].number, 5);
    assert_eq!(partitions[1].start_lba, 10_063);
    assert_eq!(partitions[1].block_count, 1000);
    assert_eq!(partitions[2].number, 6);
    assert_eq!(partitions[2].start_lba, 12_128);
    assert_eq!(partitions[2].block_count, 500);
}

#[test]
fn discovers_gpt_partitions_from_entry_array_beyond_prefix_window() {
    let mut disk = MemoryBlockIo::new(2048);
    let entry_lba = 600;
    write_protective_mbr(disk.block_mut(0));
    write_gpt_header(
        disk.block_mut(1),
        entry_lba,
        2,
        GPT_MIN_PARTITION_ENTRY_SIZE as u32,
    );
    write_gpt_entry(disk.block_mut(entry_lba as usize), 0, 700, 799);
    write_gpt_entry(
        disk.block_mut(entry_lba as usize),
        GPT_MIN_PARTITION_ENTRY_SIZE,
        1024,
        1535,
    );

    let first_block = disk.block(0).to_vec();
    let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
    let partitions = discover_gpt_partitions(shared, &first_block).expect("gpt partitions");

    assert_eq!(partitions.len(), 2);
    assert_eq!(partitions[0].number, 1);
    assert_eq!(partitions[0].start_lba, 700);
    assert_eq!(partitions[0].block_count, 100);
    assert_eq!(partitions[0].format, PartitionFormat::Gpt);
    assert_eq!(partitions[1].number, 2);
    assert_eq!(partitions[1].start_lba, 1024);
    assert_eq!(partitions[1].block_count, 512);
}

#[test]
fn discovers_gpt_partitions_on_4k_native_disk() {
    let mut disk = MemoryBlockIo::with_block_size(128, 4096);
    write_protective_mbr(disk.block_mut(0));
    write_gpt_header(disk.block_mut(1), 2, 1, GPT_MIN_PARTITION_ENTRY_SIZE as u32);
    write_gpt_entry(disk.block_mut(2), 0, 16, 63);

    let first_block = disk.block(0).to_vec();
    let shared: nextboot_fs::SharedBlockIo = Rc::new(disk);
    let partitions = discover_gpt_partitions(shared, &first_block).expect("gpt partitions");

    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].number, 1);
    assert_eq!(partitions[0].start_lba, 16);
    assert_eq!(partitions[0].block_count, 48);
    assert_eq!(partitions[0].format, PartitionFormat::Gpt);
}
