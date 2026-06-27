use super::*;

use super::*;
use alloc::rc::Rc;
use alloc::vec;

struct MemoryBlockIo {
    block_size: u32,
    data: Vec<u8>,
}

impl MemoryBlockIo {
    fn new(block_size: u32, blocks: usize) -> Self {
        Self {
            block_size,
            data: vec![0; block_size as usize * blocks],
        }
    }

    fn block_mut(&mut self, lba: usize) -> &mut [u8] {
        let block_size = self.block_size as usize;
        let start = lba * block_size;
        &mut self.data[start..start + block_size]
    }

    fn bytes_mut(&mut self, offset: usize, len: usize) -> &mut [u8] {
        &mut self.data[offset..offset + len]
    }
}

impl crate::BlockIoOps for MemoryBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        (self.data.len() / self.block_size as usize) as u64
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size as usize;
        let start = lba as usize * block_size;
        let end = start + buf.len();
        if end > self.data.len() {
            return Err(FsError::ReadError);
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

#[test]
fn reads_file_and_extents_from_minimal_ntfs() {
    let mut disk = MemoryBlockIo::new(512, 80);
    write_boot_sector(&mut disk);
    write_test_file_data(&mut disk);
    write_mft_record(
        &mut disk,
        0,
        false,
        &[data_attr_nonresident(16, &[(4, 16)])],
    );
    write_mft_record(
        &mut disk,
        5,
        true,
        &[index_root_attr(&[index_entry(
            6,
            "TEST.ISO",
            600,
            FILE_ATTRIBUTE_ARCHIVE,
        )])],
    );
    write_mft_record(
        &mut disk,
        6,
        false,
        &[data_attr_nonresident(600, &[(40, 2)])],
    );

    let fs = Ntfs::open(Rc::new(disk)).expect("open ntfs");
    let entries = fs.read_dir("/").expect("read root");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "TEST.ISO");
    assert_eq!(entries[0].size, 600);

    let info = fs.stat("/test.iso").expect("stat file");
    assert_eq!(info.start_cluster, 6);

    let extents = fs.file_extents("/TEST.ISO").expect("file extents");
    assert_eq!(extents, [FileExtent::new(0, 40, 2)]);

    let mut data = vec![0; 600];
    let read = fs.read_file("/TEST.ISO", 0, &mut data).expect("read file");
    assert_eq!(read, 600);
    assert_eq!(&data[..11], b"hello ntfs!");
    assert_eq!(data[599], 0x5A);
}

#[test]
fn detects_ntfs_boot_sector() {
    let mut disk = MemoryBlockIo::new(512, 8);
    write_boot_sector(&mut disk);
    assert_eq!(
        crate::detect_fs_type(disk.block_mut(0)),
        FileSystemType::Ntfs
    );
    assert!(is_ntfs(disk.block_mut(0)));
}

#[test]
fn follows_attribute_list_for_split_data_runs() {
    let mut disk = MemoryBlockIo::new(512, 96);
    write_boot_sector(&mut disk);
    disk.bytes_mut(50 * 512, 512)[..9].copy_from_slice(b"split-one");
    disk.bytes_mut(60 * 512, 512)[..9].copy_from_slice(b"split-two");
    disk.bytes_mut(60 * 512 + 387, 1)[0] = 0x7E;

    write_mft_record(
        &mut disk,
        0,
        false,
        &[data_attr_nonresident(24 * 512, &[(4, 24)])],
    );
    write_mft_record(
        &mut disk,
        5,
        true,
        &[index_root_attr(&[index_entry(
            7,
            "SPLIT.ISO",
            900,
            FILE_ATTRIBUTE_ARCHIVE,
        )])],
    );
    write_mft_record(
        &mut disk,
        7,
        false,
        &[
            attribute_list_attr(&[(ATTR_TYPE_DATA, 0, 7), (ATTR_TYPE_DATA, 1, 8)]),
            data_attr_nonresident_with_vcn(900, 0, &[(50, 1)]),
        ],
    );
    write_mft_record(
        &mut disk,
        8,
        false,
        &[data_attr_nonresident_with_vcn(900, 1, &[(60, 1)])],
    );

    let fs = Ntfs::open(Rc::new(disk)).expect("open ntfs");
    let extents = fs.file_extents("/split.iso").expect("file extents");
    assert_eq!(
        extents,
        [FileExtent::new(0, 50, 1), FileExtent::new(1, 60, 1)]
    );

    let mut data = vec![0; 900];
    let read = fs.read_file("/SPLIT.ISO", 0, &mut data).expect("read file");
    assert_eq!(read, 900);
    assert_eq!(&data[..9], b"split-one");
    assert_eq!(&data[512..521], b"split-two");
    assert_eq!(data[899], 0x7E);
}

fn write_boot_sector(disk: &mut MemoryBlockIo) {
    let total_blocks = (disk.data.len() / disk.block_size as usize) as u64;
    let boot = disk.block_mut(0);
    boot[0] = 0xEB;
    boot[1] = 0x52;
    boot[2] = 0x90;
    boot[3..11].copy_from_slice(NTFS_OEM_ID);
    boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
    boot[0x0D] = 1;
    boot[0x28..0x30].copy_from_slice(&total_blocks.to_le_bytes());
    boot[0x30..0x38].copy_from_slice(&4u64.to_le_bytes());
    boot[0x38..0x40].copy_from_slice(&8u64.to_le_bytes());
    boot[0x40] = (-10i8) as u8;
    boot[0x44] = (-10i8) as u8;
    boot[510] = 0x55;
    boot[511] = 0xAA;
}

fn write_test_file_data(disk: &mut MemoryBlockIo) {
    let data = disk.bytes_mut(40 * 512, 1024);
    data[..11].copy_from_slice(b"hello ntfs!");
    data[599] = 0x5A;
}

fn write_mft_record(disk: &mut MemoryBlockIo, record: usize, is_dir: bool, attrs: &[Vec<u8>]) {
    let offset = 4 * 512 + record * 1024;
    let rec = disk.bytes_mut(offset, 1024);
    rec.fill(0);
    rec[0..4].copy_from_slice(FILE_RECORD_MAGIC);
    rec[4..6].copy_from_slice(&0x30u16.to_le_bytes());
    rec[6..8].copy_from_slice(&3u16.to_le_bytes());
    rec[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
    rec[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
    rec[0x16..0x18].copy_from_slice(&(if is_dir { 3u16 } else { 1u16 }).to_le_bytes());

    let mut cursor = 0x38usize;
    for attr in attrs {
        rec[cursor..cursor + attr.len()].copy_from_slice(attr);
        cursor += attr.len();
    }
    rec[cursor..cursor + 4].copy_from_slice(&ATTR_TYPE_END.to_le_bytes());
    cursor += 4;
    rec[0x18..0x1C].copy_from_slice(&(cursor as u32).to_le_bytes());
    rec[0x1C..0x20].copy_from_slice(&1024u32.to_le_bytes());

    apply_test_fixup(rec);
}

fn apply_test_fixup(record: &mut [u8]) {
    let sequence = 0xA55Au16;
    let tail0 = u16::from_le_bytes([record[510], record[511]]);
    let tail1 = u16::from_le_bytes([record[1022], record[1023]]);
    record[0x30..0x32].copy_from_slice(&sequence.to_le_bytes());
    record[0x32..0x34].copy_from_slice(&tail0.to_le_bytes());
    record[0x34..0x36].copy_from_slice(&tail1.to_le_bytes());
    record[510..512].copy_from_slice(&sequence.to_le_bytes());
    record[1022..1024].copy_from_slice(&sequence.to_le_bytes());
}

fn data_attr_nonresident(real_size: u64, runs: &[(i64, u64)]) -> Vec<u8> {
    data_attr_nonresident_with_vcn(real_size, 0, runs)
}

fn data_attr_nonresident_with_vcn(real_size: u64, lowest_vcn: u64, runs: &[(i64, u64)]) -> Vec<u8> {
    let mut runlist = Vec::new();
    let mut previous_lcn = 0i64;
    let mut cluster_count = 0u64;
    for (lcn, len) in runs {
        let delta = *lcn - previous_lcn;
        previous_lcn = *lcn;
        cluster_count += *len;
        runlist.push(0x11);
        runlist.push(*len as u8);
        runlist.push(delta as u8);
    }
    runlist.push(0);

    let runlist_offset = 0x40usize;
    let attr_len = align_up(runlist_offset + runlist.len(), 8);
    let mut attr = vec![0; attr_len];
    attr[0..4].copy_from_slice(&ATTR_TYPE_DATA.to_le_bytes());
    attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
    attr[8] = 1;
    attr[0x10..0x18].copy_from_slice(&lowest_vcn.to_le_bytes());
    let highest_vcn = lowest_vcn + cluster_count.saturating_sub(1);
    attr[0x18..0x20].copy_from_slice(&highest_vcn.to_le_bytes());
    attr[0x20..0x22].copy_from_slice(&(runlist_offset as u16).to_le_bytes());
    attr[0x28..0x30].copy_from_slice(&real_size.to_le_bytes());
    attr[0x30..0x38].copy_from_slice(&real_size.to_le_bytes());
    attr[0x38..0x40].copy_from_slice(&real_size.to_le_bytes());
    attr[runlist_offset..runlist_offset + runlist.len()].copy_from_slice(&runlist);
    attr
}

fn attribute_list_attr(entries: &[(u32, u64, u64)]) -> Vec<u8> {
    let mut value = Vec::new();
    for (attr_type, lowest_vcn, record_number) in entries {
        let entry_len = 32usize;
        let start = value.len();
        value.resize(start + entry_len, 0);
        value[start..start + 4].copy_from_slice(&attr_type.to_le_bytes());
        value[start + 4..start + 6].copy_from_slice(&(entry_len as u16).to_le_bytes());
        value[start + 8..start + 16].copy_from_slice(&lowest_vcn.to_le_bytes());
        value[start + 16..start + 22].copy_from_slice(&(record_number.to_le_bytes()[0..6]));
        value[start + 24..start + 26].copy_from_slice(&1u16.to_le_bytes());
    }

    resident_attr(ATTR_TYPE_ATTRIBUTE_LIST, &value)
}

fn resident_attr(attr_type: u32, value: &[u8]) -> Vec<u8> {
    let value_offset = 0x18usize;
    let attr_len = align_up(value_offset + value.len(), 8);
    let mut attr = vec![0; attr_len];
    attr[0..4].copy_from_slice(&attr_type.to_le_bytes());
    attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
    attr[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
    attr[0x14..0x16].copy_from_slice(&(value_offset as u16).to_le_bytes());
    attr[value_offset..value_offset + value.len()].copy_from_slice(value);
    attr
}

fn index_root_attr(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut value = vec![0; 32];
    value[0..4].copy_from_slice(&ATTR_TYPE_FILE_NAME.to_le_bytes());
    value[8..12].copy_from_slice(&1024u32.to_le_bytes());
    value[12] = 1;
    value[16..20].copy_from_slice(&16u32.to_le_bytes());

    let mut entries_len = 0usize;
    for entry in entries {
        entries_len += entry.len();
        value.extend_from_slice(entry);
    }
    let mut last = vec![0; 16];
    last[8..10].copy_from_slice(&16u16.to_le_bytes());
    last[12..14].copy_from_slice(&INDEX_ENTRY_LAST.to_le_bytes());
    entries_len += last.len();
    value.extend_from_slice(&last);

    let total = 16 + entries_len;
    value[20..24].copy_from_slice(&(total as u32).to_le_bytes());
    value[24..28].copy_from_slice(&(total as u32).to_le_bytes());

    let value_offset = 0x18usize;
    let attr_len = align_up(value_offset + value.len(), 8);
    let mut attr = vec![0; attr_len];
    attr[0..4].copy_from_slice(&ATTR_TYPE_INDEX_ROOT.to_le_bytes());
    attr[4..8].copy_from_slice(&(attr_len as u32).to_le_bytes());
    attr[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
    attr[0x14..0x16].copy_from_slice(&(value_offset as u16).to_le_bytes());
    attr[value_offset..value_offset + value.len()].copy_from_slice(&value);
    attr
}

fn index_entry(record: u64, name: &str, size: u64, attrs: u32) -> Vec<u8> {
    let mut file_name = vec![0; 66 + name.encode_utf16().count() * 2];
    file_name[40..48].copy_from_slice(&align_up(size as usize, 512).to_le_bytes());
    file_name[48..56].copy_from_slice(&size.to_le_bytes());
    file_name[56..60].copy_from_slice(&attrs.to_le_bytes());
    file_name[64] = name.encode_utf16().count() as u8;
    file_name[65] = 1;
    for (index, ch) in name.encode_utf16().enumerate() {
        let offset = 66 + index * 2;
        file_name[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
    }

    let entry_len = align_up(16 + file_name.len(), 8);
    let mut entry = vec![0; entry_len];
    entry[0..6].copy_from_slice(&(record as u64).to_le_bytes()[0..6].as_ref());
    entry[8..10].copy_from_slice(&(entry_len as u16).to_le_bytes());
    entry[10..12].copy_from_slice(&(file_name.len() as u16).to_le_bytes());
    entry[16..16 + file_name.len()].copy_from_slice(&file_name);
    entry
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) / align * align
}
