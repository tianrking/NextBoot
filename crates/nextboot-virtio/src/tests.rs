use super::*;
use crate::mapping::ByteMappingTable;

fn fill_lba_read(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
    for b in buf.iter_mut() {
        *b = lba as u8;
    }
    Ok(())
}

fn patterned_4k_read(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
    for (index, b) in buf.iter_mut().enumerate() {
        *b = (lba as u8)
            .wrapping_mul(16)
            .wrapping_add((index / 1024) as u8);
    }
    Ok(())
}

#[test]
fn test_virtual_device_config() {
    let config = VirtualDeviceConfig::new(
        VirtualDeviceType::DvdRom,
        1000,
        1024 * 1024 * 700, // 700 MB
        2048,
    );

    assert_eq!(config.block_count(), 358400);
    assert_eq!(config.device_type, VirtualDeviceType::DvdRom);
}

#[test]
fn virtual_device_config_keeps_cdrom_boot_info() {
    let boot = CdRomBootInfo::new(2, 48, 0);
    let config =
        VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 4096, 2048).with_cdrom_boot(boot);

    assert_eq!(config.cdrom_boot, Some(CdRomBootInfo::new(2, 48, 1)));
}

#[test]
fn test_virtual_block_io() {
    let config = VirtualDeviceConfig::new(
        VirtualDeviceType::HardDisk,
        1000,
        1024 * 1024, // 1 MB
        512,
    );

    let mut vbio = VirtualBlockIo::new(config);

    // 设置物理读取函数
    vbio.set_physical_read(|_lba, buf| {
        // 模拟读取
        for b in buf.iter_mut() {
            *b = 0xAA;
        }
        Ok(())
    });

    // 测试读取
    let mut buf = [0u8; 512];
    let result = vbio.read_blocks(vbio.media_id(), 0, &mut buf);
    assert!(result.is_ok());

    // 测试写入 (应该失败)
    let result = vbio.write_blocks(vbio.media_id(), 0, &[0u8; 512]);
    assert!(result.is_err());
}

#[test]
fn test_virtual_block_io_reads_fragmented_file_extents() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 1024, 512);
    let extents = [(0, 10, 1), (1, 20, 1)];
    let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
    vbio.set_physical_read(fill_lba_read);

    let mut buf = [0u8; 1024];
    vbio.read_blocks(vbio.media_id(), 0, &mut buf)
        .expect("fragmented extent read");

    assert!(buf[..512].iter().all(|b| *b == 10));
    assert!(buf[512..].iter().all(|b| *b == 20));
}

#[test]
fn test_virtual_block_io_maps_2048_virtual_blocks_to_4k_physical_blocks() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 8192, 2048)
        .with_physical_block_size(4096);
    let extents = [(0, 2, 2)];
    let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
    vbio.set_physical_read(patterned_4k_read);

    let mut buf = [0u8; 8192];
    vbio.read_blocks(vbio.media_id(), 0, &mut buf)
        .expect("4K-backed DVD read");

    assert_eq!(buf[0], 32);
    assert_eq!(buf[1023], 32);
    assert_eq!(buf[1024], 33);
    assert_eq!(buf[2048], 34);
    assert_eq!(buf[3072], 35);
    assert_eq!(buf[4096], 48);
    assert_eq!(buf[5120], 49);
    assert_eq!(buf[7168], 51);
}

#[test]
fn test_virtual_block_io_zero_fills_file_tail_padding() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 600, 512);
    let extents = [(0, 7, 2)];
    let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
    vbio.set_physical_read(fill_lba_read);

    let mut buf = [0xFFu8; 1024];
    vbio.read_blocks(vbio.media_id(), 0, &mut buf)
        .expect("tail padding read");

    assert!(buf[..512].iter().all(|b| *b == 7));
    assert!(buf[512..600].iter().all(|b| *b == 8));
    assert!(buf[600..].iter().all(|b| *b == 0));
}

#[test]
fn test_virtual_block_io_zero_fills_sparse_byte_mapping() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 1536, 512);
    let mut byte_mapping = ByteMappingTable::empty();
    byte_mapping.add_segment(0, 512, 10 * 512);
    byte_mapping.add_segment(1024, 512, 20 * 512);
    byte_mapping.truncate(1536);
    let mut vbio = VirtualBlockIo::with_byte_mapping(config, byte_mapping);
    vbio.set_physical_read(fill_lba_read);

    let mut buf = [0xFFu8; 1536];
    vbio.read_blocks(vbio.media_id(), 0, &mut buf)
        .expect("sparse byte mapping read");

    assert!(buf[..512].iter().all(|b| *b == 10));
    assert!(buf[512..1024].iter().all(|b| *b == 0));
    assert!(buf[1024..].iter().all(|b| *b == 20));
}

#[test]
fn test_virtual_block_io_applies_memory_overlays() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 512, 512);
    let extents = [(0, 10, 1)];
    let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
    vbio.set_physical_read(fill_lba_read);
    vbio.add_memory_overlay(MemoryOverlay::new(4, alloc::vec![1, 2, 3, 4]))
        .expect("in-place overlay");
    vbio.add_memory_overlay(MemoryOverlay::new(512, alloc::vec![9, 8, 7]))
        .expect("appended overlay");

    let mut first = [0u8; 8];
    vbio.read_bytes(vbio.media_id(), 0, &mut first)
        .expect("overlay byte read");
    assert_eq!(first, [10, 10, 10, 10, 1, 2, 3, 4]);

    let mut appended = [0u8; 3];
    vbio.read_bytes(vbio.media_id(), 512, &mut appended)
        .expect("appended overlay read");
    assert_eq!(appended, [9, 8, 7]);
    assert_eq!(vbio.device_info().size_bytes, 515);
}

#[test]
fn test_virtual_block_io_reads_unaligned_bytes() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 4096, 2048)
        .with_physical_block_size(4096);
    let extents = [(0, 2, 1)];
    let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
    vbio.set_physical_read(patterned_4k_read);

    let mut buf = [0u8; 3];
    vbio.read_bytes(vbio.media_id(), 1023, &mut buf)
        .expect("unaligned byte read");

    assert_eq!(buf, [32, 33, 33]);
}

#[test]
fn test_virtual_block_io_rejects_out_of_range_byte_read() {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 600, 512);
    let mut vbio = VirtualBlockIo::new(config);
    vbio.set_physical_read(fill_lba_read);

    let mut buf = [0u8; 2];
    assert!(matches!(
        vbio.read_bytes(vbio.media_id(), 599, &mut buf),
        Err(VirtIoError::OutOfBounds)
    ));
}
