use super::*;
use crate::exfat::ExFat;
use crate::fat32::Fat32;
use crate::iso9660::{
    detect_udf_volume, get_eltorito_boot_info, read_efi_eltorito_boot_info, Iso9660,
};
use crate::udf::Udf;
use alloc::rc::Rc;
use alloc::vec;

#[path = "tests/support.rs"]
mod support;
use support::*;

#[test]
fn read_full_blocks_checks_bounds_and_alignment() {
    let io = MemoryBlockIo::new(512, 2);
    let mut one_block = vec![0u8; 512];
    assert!(read_full_blocks(&io, 0, &mut one_block).is_ok());

    let mut partial = vec![0u8; 128];
    assert!(matches!(
        read_full_blocks(&io, 0, &mut partial),
        Err(FsError::InvalidArgument)
    ));

    let mut too_far = vec![0u8; 512];
    assert!(matches!(
        read_full_blocks(&io, 2, &mut too_far),
        Err(FsError::ReadError)
    ));
}

#[test]
fn iso9660_reads_directory_entries_and_file_data() {
    let mut io = MemoryBlockIo::new(2048, 32);

    {
        let pvd = io.block_mut(16);
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        pvd[40..48].copy_from_slice(b"NEXTBOOT");
        pvd[84..88].copy_from_slice(&32u32.to_le_bytes());
        pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
        write_iso_record(pvd, 156, 20, 2048, 0x02, &[0]);
    }

    {
        let end = io.block_mut(17);
        end[0] = 255;
        end[1..6].copy_from_slice(b"CD001");
        end[6] = 1;
    }

    write_iso_record(io.block_mut(20), 0, 21, 11, 0x00, b"KERNEL.;1");
    io.block_mut(21)[..11].copy_from_slice(b"hello world");

    let fs = Iso9660::open(Rc::new(io)).expect("valid ISO9660 filesystem");
    let entries = fs.read_dir("/").expect("root directory");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "kernel");
    assert_eq!(entries[0].size, 11);

    let mut data = [0u8; 11];
    let read = fs.read_file("/kernel", 0, &mut data).expect("file read");
    assert_eq!(read, 11);
    assert_eq!(&data, b"hello world");
}

#[test]
fn eltorito_reads_efi_default_entry() {
    let mut io = MemoryBlockIo::new(2048, 40);
    write_el_torito_boot_record(io.block_mut(17), 22);
    write_validation_entry(io.block_mut(22), 0xEF);
    write_boot_entry(io.block_mut(22), 32, 0, 4, 30);

    let info = read_efi_eltorito_boot_info(&io)
        .expect("read catalog")
        .expect("efi boot entry");

    assert_eq!(info.catalog_lba, 22);
    assert_eq!(info.boot_entry, 0);
    assert_eq!(info.platform_id, 0xEF);
    assert_eq!(info.image_lba, 30);
    assert_eq!(info.image_block_count_2048(), 1);
    assert_eq!(get_eltorito_boot_info(&io.data), Some((30, 4)));
}

#[test]
fn eltorito_prefers_efi_section_entry() {
    let mut io = MemoryBlockIo::new(2048, 64);
    write_el_torito_boot_record(io.block_mut(17), 24);

    {
        let catalog = io.block_mut(24);
        write_validation_entry(catalog, 0x00);
        write_boot_entry(catalog, 32, 0, 4, 31);

        catalog[64] = 0x91;
        catalog[65] = 0xEF;
        catalog[66..68].copy_from_slice(&1u16.to_le_bytes());
        write_boot_entry(catalog, 96, 0, 8, 42);
    }

    let info = read_efi_eltorito_boot_info(&io)
        .expect("read catalog")
        .expect("efi section boot entry");

    assert_eq!(info.boot_entry, 1);
    assert_eq!(info.platform_id, 0xEF);
    assert_eq!(info.image_lba, 42);
    assert_eq!(info.sector_count, 8);
    assert_eq!(info.image_block_count_2048(), 2);
}

#[test]
fn detects_udf_volume_after_iso_terminator() {
    let mut io = MemoryBlockIo::new(2048, 40);

    {
        let terminator = io.block_mut(18);
        terminator[0] = 255;
        terminator[1..6].copy_from_slice(b"CD001");
    }
    io.block_mut(19)[1..6].copy_from_slice(b"BEA01");
    io.block_mut(20)[1..6].copy_from_slice(b"NSR03");

    assert_eq!(detect_udf_volume(&io).expect("udf probe"), true);
}

#[test]
fn rejects_udf_probe_without_nsr_descriptor() {
    let mut io = MemoryBlockIo::new(2048, 40);

    {
        let terminator = io.block_mut(18);
        terminator[0] = 255;
        terminator[1..6].copy_from_slice(b"CD001");
    }
    io.block_mut(19)[1..6].copy_from_slice(b"BEA01");

    assert_eq!(detect_udf_volume(&io).expect("udf probe"), false);
}

#[test]
fn udf_reads_directories_files_and_extents() {
    let fs = Udf::open(Rc::new(udf_fixture())).expect("valid UDF filesystem");

    let root = fs.read_dir("/").expect("root directory");
    assert_eq!(root.len(), 1);
    assert_eq!(root[0].name, "EFI");
    assert!(root[0].is_dir);

    let stat = fs.stat("/efi/bootx64.efi").expect("file stat");
    assert_eq!(stat.size, 14);
    assert!(!stat.is_dir);

    let mut data = [0u8; 14];
    let read = fs
        .read_file("/EFI/BOOTX64.EFI", 0, &mut data)
        .expect("file read");
    assert_eq!(read, data.len());
    assert_eq!(&data, b"hello udf boot");

    let extents = fs.file_extents("/efi/bootx64.efi").expect("extents");
    assert_eq!(extents, vec![FileExtent::new(0, 107, 1)]);
}

#[test]
fn udf_replacement_patch_redirects_file_entry_to_appended_data() {
    let fs = Udf::open(Rc::new(udf_fixture())).expect("valid UDF filesystem");
    let patch = fs
        .file_replacement_patch("/EFI/BOOTX64.EFI", 108, 11, 2048)
        .expect("replacement patch");

    assert_eq!(patch.file_entry_offset, 106 * 2048);
    assert!(patch.partition_descriptor.is_none());

    let mut io = udf_fixture();
    apply_udf_replacement_patch(&mut io, patch);
    io.block_mut(108)[..11].copy_from_slice(b"hello patch");

    let fs = Udf::open(Rc::new(io)).expect("patched UDF filesystem");
    let stat = fs.stat("/efi/bootx64.efi").expect("file stat");
    assert_eq!(stat.size, 11);

    let extents = fs.file_extents("/efi/bootx64.efi").expect("extents");
    assert_eq!(extents, vec![FileExtent::new(0, 108, 1)]);

    let mut data = [0u8; 11];
    let read = fs
        .read_file("/EFI/BOOTX64.EFI", 0, &mut data)
        .expect("file read");
    assert_eq!(read, data.len());
    assert_eq!(&data, b"hello patch");
}

#[test]
fn udf_replacement_patch_extends_partition_descriptor_when_needed() {
    let fs = Udf::open(Rc::new(udf_fixture())).expect("valid UDF filesystem");
    let patch = fs
        .file_replacement_patch("/EFI/BOOTX64.EFI", 240, 3000, 4096)
        .expect("replacement patch");
    let descriptor = patch
        .partition_descriptor
        .as_ref()
        .expect("partition descriptor patch");

    assert_eq!(descriptor.descriptor_offset, 32 * 2048);
    assert_eq!(
        u32::from_le_bytes([
            descriptor.descriptor_data[192],
            descriptor.descriptor_data[193],
            descriptor.descriptor_data[194],
            descriptor.descriptor_data[195],
        ]),
        142
    );

    let mut io = udf_fixture();
    apply_udf_replacement_patch(&mut io, patch);
    let replacement = vec![b'X'; 3000];
    let replacement_start = 240 * 2048;
    io.data[replacement_start..replacement_start + replacement.len()].copy_from_slice(&replacement);

    let fs = Udf::open(Rc::new(io)).expect("patched UDF filesystem");
    let mut data = vec![0u8; 3000];
    let read = fs
        .read_file("/EFI/BOOTX64.EFI", 0, &mut data)
        .expect("file read");
    assert_eq!(read, data.len());
    assert_eq!(data, replacement);
}

#[test]
fn fat32_file_extents_follow_fragmented_cluster_chain() {
    let mut io = MemoryBlockIo::new(512, 16);

    {
        let boot = io.block_mut(0);
        boot[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        boot[3..11].copy_from_slice(b"NEXTBOOT");
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 1;
        boot[14..16].copy_from_slice(&1u16.to_le_bytes());
        boot[16] = 1;
        boot[32..36].copy_from_slice(&16u32.to_le_bytes());
        boot[36..40].copy_from_slice(&1u32.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());
        boot[82..90].copy_from_slice(b"FAT32   ");
        boot[510] = 0x55;
        boot[511] = 0xAA;
    }

    {
        let fat = io.block_mut(1);
        fat[0..4].copy_from_slice(&0x0FFFFFF8u32.to_le_bytes());
        fat[4..8].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
        fat[8..12].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
        fat[12..16].copy_from_slice(&5u32.to_le_bytes());
        fat[20..24].copy_from_slice(&0x0FFFFFFFu32.to_le_bytes());
    }

    {
        let root = io.block_mut(2);
        root[0..11].copy_from_slice(b"TEST    ISO");
        root[11] = FileAttributes::ARCHIVE.bits();
        root[26..28].copy_from_slice(&3u16.to_le_bytes());
        root[28..32].copy_from_slice(&700u32.to_le_bytes());
    }

    let fs = Fat32::open(Rc::new(io)).expect("valid FAT32 filesystem");
    let extents = fs.file_extents("/TEST.ISO").expect("file extents");

    assert_eq!(
        extents,
        vec![FileExtent::new(0, 3, 1), FileExtent::new(1, 5, 1),]
    );
}

#[test]
fn detects_standard_fat32_filesystem_type_field() {
    let mut boot = [0u8; 512];
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510] = 0x55;
    boot[511] = 0xAA;

    assert_eq!(detect_fs_type(&boot), FileSystemType::Fat32);
    assert!(crate::fat32::is_fat32(&boot));
}

#[test]
fn exfat_file_extents_support_no_fat_chain_files() {
    let mut io = MemoryBlockIo::new(512, 16);

    {
        let boot = io.block_mut(0);
        boot[0..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
        boot[3..11].copy_from_slice(b"EXFAT   ");
        boot[72..80].copy_from_slice(&16u64.to_le_bytes());
        boot[80..84].copy_from_slice(&1u32.to_le_bytes());
        boot[84..88].copy_from_slice(&1u32.to_le_bytes());
        boot[88..92].copy_from_slice(&2u32.to_le_bytes());
        boot[92..96].copy_from_slice(&14u32.to_le_bytes());
        boot[96..100].copy_from_slice(&2u32.to_le_bytes());
        boot[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
        boot[108] = 9;
        boot[109] = 0;
        boot[110] = 1;
        boot[510..512].copy_from_slice(&0xAA55u16.to_le_bytes());
    }

    {
        let fat = io.block_mut(1);
        fat[8..12].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    }

    {
        let root = io.block_mut(2);
        root[0] = 0x85;
        root[1] = 2;
        root[4..6].copy_from_slice(&0x20u16.to_le_bytes());

        root[32] = 0xC0;
        root[33] = 0x02;
        root[35] = 8;
        root[52..56].copy_from_slice(&4u32.to_le_bytes());
        root[56..64].copy_from_slice(&1024u64.to_le_bytes());

        root[64] = 0xC1;
        write_utf16_name(&mut root[64..96], "TEST.ISO");
    }

    io.block_mut(4)[..5].copy_from_slice(b"first");
    io.block_mut(5)[..6].copy_from_slice(b"second");

    let fs = ExFat::open(Rc::new(io)).expect("valid exFAT filesystem");
    let extents = fs.file_extents("/TEST.ISO").expect("file extents");

    assert_eq!(extents, vec![FileExtent::new(0, 4, 2)]);

    let mut data = vec![0u8; 518];
    let read = fs.read_file("/TEST.ISO", 0, &mut data).expect("file read");
    assert_eq!(read, 518);
    assert_eq!(&data[..5], b"first");
    assert_eq!(&data[512..518], b"second");
}
