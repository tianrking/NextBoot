use super::*;
use alloc::vec::Vec;

pub(super) struct MemoryBlockIo {
    block_size: u32,
    pub(super) data: Vec<u8>,
}

impl MemoryBlockIo {
    pub(super) fn new(block_size: u32, blocks: usize) -> Self {
        Self {
            block_size,
            data: vec![0; block_size as usize * blocks],
        }
    }

    pub(super) fn block_mut(&mut self, lba: usize) -> &mut [u8] {
        let block_size = self.block_size as usize;
        let start = lba * block_size;
        &mut self.data[start..start + block_size]
    }
}

impl BlockIoOps for MemoryBlockIo {
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

pub(super) fn write_iso_record(
    block: &mut [u8],
    offset: usize,
    lba: u32,
    size: u32,
    flags: u8,
    name: &[u8],
) {
    let len = 33 + name.len();
    block[offset] = len as u8;
    block[offset + 2..offset + 6].copy_from_slice(&lba.to_le_bytes());
    block[offset + 10..offset + 14].copy_from_slice(&size.to_le_bytes());
    block[offset + 25] = flags;
    block[offset + 28..offset + 30].copy_from_slice(&1u16.to_le_bytes());
    block[offset + 32] = name.len() as u8;
    block[offset + 33..offset + 33 + name.len()].copy_from_slice(name);
}

pub(super) fn write_utf16_name(entry: &mut [u8], name: &str) {
    for (i, ch) in name.encode_utf16().enumerate() {
        let offset = 2 + i * 2;
        entry[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
    }
}

pub(super) fn write_el_torito_boot_record(block: &mut [u8], catalog_lba: u32) {
    block[0] = 0;
    block[1..6].copy_from_slice(b"CD001");
    block[6] = 1;
    block[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
    block[0x47..0x4B].copy_from_slice(&catalog_lba.to_le_bytes());
}

pub(super) fn write_validation_entry(catalog: &mut [u8], platform_id: u8) {
    catalog[0] = 0x01;
    catalog[1] = platform_id;
    catalog[30] = 0x55;
    catalog[31] = 0xAA;
}

pub(super) fn write_boot_entry(
    catalog: &mut [u8],
    offset: usize,
    media_type: u8,
    sector_count: u16,
    image_lba: u32,
) {
    catalog[offset] = 0x88;
    catalog[offset + 1] = media_type;
    catalog[offset + 6..offset + 8].copy_from_slice(&sector_count.to_le_bytes());
    catalog[offset + 8..offset + 12].copy_from_slice(&image_lba.to_le_bytes());
}

pub(super) fn write_udf_tag(block: &mut [u8], ident: u16, location: u32) {
    block[0..2].copy_from_slice(&ident.to_le_bytes());
    block[2..4].copy_from_slice(&2u16.to_le_bytes());
    block[12..16].copy_from_slice(&location.to_le_bytes());
}

pub(super) fn write_udf_long_ad(block: &mut [u8], offset: usize, length: u32, block_num: u32) {
    block[offset..offset + 4].copy_from_slice(&length.to_le_bytes());
    block[offset + 4..offset + 8].copy_from_slice(&block_num.to_le_bytes());
    block[offset + 8..offset + 10].copy_from_slice(&0u16.to_le_bytes());
}

pub(super) fn write_udf_short_ad(block: &mut [u8], offset: usize, length: u32, position: u32) {
    block[offset..offset + 4].copy_from_slice(&length.to_le_bytes());
    block[offset + 4..offset + 8].copy_from_slice(&position.to_le_bytes());
}

pub(super) fn write_udf_file_entry(
    block: &mut [u8],
    location: u32,
    file_type: u8,
    file_size: u64,
    alloc_desc_len: u32,
) {
    write_udf_tag(block, 0x0105, location);
    block[27] = file_type;
    block[34..36].copy_from_slice(&0u16.to_le_bytes());
    block[56..64].copy_from_slice(&file_size.to_le_bytes());
    block[172..176].copy_from_slice(&alloc_desc_len.to_le_bytes());
}

pub(super) fn write_udf_fid(
    block: &mut [u8],
    offset: usize,
    name: &str,
    icb_block: u32,
    is_dir: bool,
) -> usize {
    let raw_len = 1 + name.len();
    write_udf_tag(&mut block[offset..], 0x0101, 0);
    block[offset + 16..offset + 18].copy_from_slice(&1u16.to_le_bytes());
    block[offset + 18] = if is_dir { 0x02 } else { 0 };
    block[offset + 19] = raw_len as u8;
    write_udf_long_ad(block, offset + 20, 2048, icb_block);
    block[offset + 36..offset + 38].copy_from_slice(&0u16.to_le_bytes());
    block[offset + 38] = 8;
    block[offset + 39..offset + 39 + name.len()].copy_from_slice(name.as_bytes());
    let end = offset + 38 + raw_len;
    (end + 3) & !3
}

pub(super) fn udf_fixture() -> MemoryBlockIo {
    let mut io = MemoryBlockIo::new(2048, 320);

    {
        let anchor = io.block_mut(256);
        write_udf_tag(anchor, 0x0002, 256);
        anchor[16..20].copy_from_slice(&(4u32 * 2048).to_le_bytes());
        anchor[20..24].copy_from_slice(&32u32.to_le_bytes());
    }

    {
        let pd = io.block_mut(32);
        write_udf_tag(pd, 0x0005, 32);
        pd[22..24].copy_from_slice(&0u16.to_le_bytes());
        pd[188..192].copy_from_slice(&100u32.to_le_bytes());
        pd[192..196].copy_from_slice(&100u32.to_le_bytes());
    }

    {
        let lvd = io.block_mut(33);
        write_udf_tag(lvd, 0x0006, 33);
        lvd[212..216].copy_from_slice(&2048u32.to_le_bytes());
        write_udf_long_ad(lvd, 248, 2048, 1);
        lvd[264..268].copy_from_slice(&6u32.to_le_bytes());
        lvd[268..272].copy_from_slice(&1u32.to_le_bytes());
        lvd[440] = 1;
        lvd[441] = 6;
        lvd[442..444].copy_from_slice(&0u16.to_le_bytes());
        lvd[444..446].copy_from_slice(&0u16.to_le_bytes());
    }

    write_udf_tag(io.block_mut(34), 0x0008, 34);

    {
        let fsd = io.block_mut(101);
        write_udf_tag(fsd, 0x0100, 1);
        write_udf_long_ad(fsd, 400, 2048, 2);
    }

    let root_dir_len = write_udf_fid(io.block_mut(103), 0, "EFI", 4, true) as u64;
    {
        let root_fe = io.block_mut(102);
        write_udf_file_entry(root_fe, 2, 0x04, root_dir_len, 8);
        write_udf_short_ad(root_fe, 176, root_dir_len as u32, 3);
    }

    let efi_dir_len = write_udf_fid(io.block_mut(105), 0, "BOOTX64.EFI", 6, false) as u64;
    {
        let efi_fe = io.block_mut(104);
        write_udf_file_entry(efi_fe, 4, 0x04, efi_dir_len, 8);
        write_udf_short_ad(efi_fe, 176, efi_dir_len as u32, 5);
    }

    let file_data = b"hello udf boot";
    io.block_mut(107)[..file_data.len()].copy_from_slice(file_data);
    {
        let file_fe = io.block_mut(106);
        write_udf_file_entry(file_fe, 6, 0x05, file_data.len() as u64, 8);
        write_udf_short_ad(file_fe, 176, file_data.len() as u32, 7);
    }

    io
}

pub(super) fn apply_udf_replacement_patch(
    io: &mut MemoryBlockIo,
    patch: crate::udf::UdfFileReplacementPatch,
) {
    let entry_start = patch.file_entry_offset as usize;
    let entry_end = entry_start + patch.file_entry_data.len();
    io.data[entry_start..entry_end].copy_from_slice(&patch.file_entry_data);

    if let Some(descriptor) = patch.partition_descriptor {
        let descriptor_start = descriptor.descriptor_offset as usize;
        let descriptor_end = descriptor_start + descriptor.descriptor_data.len();
        io.data[descriptor_start..descriptor_end].copy_from_slice(&descriptor.descriptor_data);
    }
}
