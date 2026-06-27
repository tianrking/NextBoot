use super::*;
use crate::{VirtualBlockIo, VirtualDeviceConfig, VirtualDeviceType};

fn fill_from_lba(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
    for byte in buf {
        *byte = lba as u8;
    }
    Ok(())
}

fn make_protocol() -> VirtualBlockIoProtocol {
    let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 10, 1024, 512);
    let mut vbio = VirtualBlockIo::new(config);
    vbio.set_physical_read(fill_from_lba);
    VirtualBlockIoProtocol::new(vbio)
}

#[test]
fn read_blocks_handler_delegates_to_virtual_block_io() {
    let mut protocol = make_protocol();
    let ptr = protocol.as_ptr();
    let media_id = protocol.media.media_id;
    let mut buf = [0u8; 1024];

    let status = unsafe {
        ((*ptr).read_blocks)(ptr, media_id, 0, buf.len() as u64, buf.as_mut_ptr().cast())
    };

    assert_eq!(status, UefiStatus::Success as u64);
    assert!(buf[..512].iter().all(|byte| *byte == 10));
    assert!(buf[512..].iter().all(|byte| *byte == 11));
}

#[test]
fn read_blocks_ex_handler_updates_token_and_reads_blocks() {
    let mut protocol = make_protocol();
    let ptr = protocol.block_io_2_ptr();
    let media_id = protocol.media.media_id;
    let mut token = BlockIo2Token {
        event: core::ptr::null_mut(),
        transaction_status: UefiStatus::DeviceError as u64,
    };
    let mut buf = [0u8; 1024];

    let status = unsafe {
        ((*ptr).read_blocks_ex)(
            ptr,
            media_id,
            0,
            &mut token,
            buf.len(),
            buf.as_mut_ptr().cast(),
        )
    };

    assert_eq!(status, UefiStatus::Success as u64);
    assert_eq!(token.transaction_status, UefiStatus::Success as u64);
    assert!(buf[..512].iter().all(|byte| *byte == 10));
    assert!(buf[512..].iter().all(|byte| *byte == 11));
}

#[test]
fn read_blocks_handler_reports_media_change_and_bad_buffer_size() {
    let mut protocol = make_protocol();
    let ptr = protocol.as_ptr();
    let mut buf = [0u8; 512];

    let wrong_media = unsafe {
        ((*ptr).read_blocks)(
            ptr,
            0xDEAD_BEEF,
            0,
            buf.len() as u64,
            buf.as_mut_ptr().cast(),
        )
    };
    assert_eq!(wrong_media, UefiStatus::MediaChanged as u64);

    let bad_size = unsafe {
        ((*ptr).read_blocks)(ptr, protocol.media.media_id, 0, 7, buf.as_mut_ptr().cast())
    };
    assert_eq!(bad_size, UefiStatus::BadBufferSize as u64);
}

#[test]
fn read_blocks_ex_handler_reports_bad_buffer_size_in_token() {
    let mut protocol = make_protocol();
    let ptr = protocol.block_io_2_ptr();
    let mut token = BlockIo2Token {
        event: core::ptr::null_mut(),
        transaction_status: UefiStatus::Success as u64,
    };
    let mut buf = [0u8; 8];

    let status = unsafe {
        ((*ptr).read_blocks_ex)(
            ptr,
            protocol.media.media_id,
            0,
            &mut token,
            7,
            buf.as_mut_ptr().cast(),
        )
    };

    assert_eq!(status, UefiStatus::BadBufferSize as u64);
    assert_eq!(token.transaction_status, UefiStatus::BadBufferSize as u64);
}

#[test]
fn write_blocks_handler_stays_write_protected() {
    let mut protocol = make_protocol();
    let ptr = protocol.as_ptr();
    let buf = [0u8; 512];

    let status = unsafe {
        ((*ptr).write_blocks)(
            ptr,
            protocol.media.media_id,
            0,
            buf.len() as u64,
            buf.as_ptr().cast(),
        )
    };

    assert_eq!(status, UefiStatus::WriteProtected as u64);
}

#[test]
fn read_disk_handler_supports_unaligned_byte_reads() {
    let mut protocol = make_protocol();
    let ptr = protocol.disk_io_ptr();
    let media_id = protocol.media.media_id;
    let mut buf = [0u8; 2];

    let status =
        unsafe { ((*ptr).read_disk)(ptr, media_id, 511, buf.len(), buf.as_mut_ptr().cast()) };

    assert_eq!(status, UefiStatus::Success as u64);
    assert_eq!(buf, [10, 11]);
}

#[test]
fn read_disk_ex_handler_supports_unaligned_byte_reads() {
    let mut protocol = make_protocol();
    let ptr = protocol.disk_io_2_ptr();
    let media_id = protocol.media.media_id;
    let mut token = DiskIo2Token {
        event: core::ptr::null_mut(),
        transaction_status: UefiStatus::DeviceError as u64,
    };
    let mut buf = [0u8; 2];

    let status = unsafe {
        ((*ptr).read_disk_ex)(
            ptr,
            media_id,
            511,
            &mut token,
            buf.len(),
            buf.as_mut_ptr().cast(),
        )
    };

    assert_eq!(status, UefiStatus::Success as u64);
    assert_eq!(token.transaction_status, UefiStatus::Success as u64);
    assert_eq!(buf, [10, 11]);
}

#[test]
fn write_disk_handler_stays_write_protected() {
    let mut protocol = make_protocol();
    let ptr = protocol.disk_io_ptr();
    let media_id = protocol.media.media_id;
    let buf = [0u8; 3];

    let status = unsafe { ((*ptr).write_disk)(ptr, media_id, 7, buf.len(), buf.as_ptr().cast()) };

    assert_eq!(status, UefiStatus::WriteProtected as u64);
}

#[test]
fn write_disk_ex_handler_stays_write_protected() {
    let mut protocol = make_protocol();
    let ptr = protocol.disk_io_2_ptr();
    let media_id = protocol.media.media_id;
    let mut token = DiskIo2Token {
        event: core::ptr::null_mut(),
        transaction_status: UefiStatus::Success as u64,
    };
    let buf = [0u8; 3];

    let status = unsafe {
        ((*ptr).write_disk_ex)(ptr, media_id, 7, &mut token, buf.len(), buf.as_ptr().cast())
    };

    assert_eq!(status, UefiStatus::WriteProtected as u64);
    assert_eq!(token.transaction_status, UefiStatus::WriteProtected as u64);
}

#[test]
fn cdrom_device_path_has_media_node_and_end_node() {
    let path = create_cdrom_device_path(0, 0, 128);

    assert_eq!(path.len(), core::mem::size_of::<CdRomDevicePath>() + 4);
    assert_eq!(path[0], DevicePathType::MEDIA.bits());
    assert_eq!(path[1], MediaSubtype::CdRom as u8);
    assert_eq!(u16::from_le_bytes([path[2], path[3]]), 24);
    assert_eq!(path[path.len() - 4], DevicePathType::END.bits());
    assert_eq!(path[path.len() - 3], 0xFF);
    assert_eq!(
        u16::from_le_bytes([path[path.len() - 2], path[path.len() - 1]]),
        4
    );
}

#[test]
fn virtual_disk_controller_device_path_has_vendor_node_and_end_node() {
    let path = create_virtual_disk_controller_device_path();

    assert_eq!(
        path.len(),
        core::mem::size_of::<VendorHardwareDevicePath>() + 4
    );
    assert_eq!(path[0], DevicePathType::HARDWARE.bits());
    assert_eq!(path[1], HardwareSubtype::Vendor as u8);
    assert_eq!(u16::from_le_bytes([path[2], path[3]]), 20);
    assert_eq!(&path[4..20], &NEXTBOOT_VIRTUAL_DISK_GUID);
    assert_eq!(path[path.len() - 4], DevicePathType::END.bits());
    assert_eq!(path[path.len() - 3], 0xFF);
    assert_eq!(
        u16::from_le_bytes([path[path.len() - 2], path[path.len() - 1]]),
        4
    );
}

#[test]
fn hard_drive_device_path_has_media_node_and_end_node() {
    let path = create_hard_drive_device_path(1, 0, 128);

    assert_eq!(path.len(), core::mem::size_of::<HardDriveDevicePath>() + 4);
    assert_eq!(path[0], DevicePathType::MEDIA.bits());
    assert_eq!(path[1], MediaSubtype::HardDrive as u8);
    assert_eq!(u16::from_le_bytes([path[2], path[3]]), 42);
    assert_eq!(path[path.len() - 4], DevicePathType::END.bits());
    assert_eq!(path[path.len() - 3], 0xFF);
    assert_eq!(
        u16::from_le_bytes([path[path.len() - 2], path[path.len() - 1]]),
        4
    );
}

#[test]
fn append_file_path_device_path_replaces_end_node() {
    let base = create_cdrom_device_path(0, 0, 128);
    let full = append_file_path_device_path(&base, "/EFI/BOOT/BOOTX64.EFI").unwrap();
    let cdrom_len = core::mem::size_of::<CdRomDevicePath>();
    let file_node_len = u16::from_le_bytes([full[cdrom_len + 2], full[cdrom_len + 3]]) as usize;

    assert_eq!(full[cdrom_len], DevicePathType::MEDIA.bits());
    assert_eq!(full[cdrom_len + 1], MediaSubtype::FilePath as u8);
    assert_eq!(
        file_node_len,
        4 + ("\\EFI\\BOOT\\BOOTX64.EFI".len() + 1) * 2
    );
    assert_eq!(full[full.len() - 4], DevicePathType::END.bits());
    assert_eq!(full[full.len() - 3], 0xFF);
    assert_eq!(
        u16::from_le_bytes([full[full.len() - 2], full[full.len() - 1]]),
        4
    );
    assert_eq!(full.len(), cdrom_len + file_node_len + 4);
}

#[test]
fn append_file_path_device_path_adds_leading_backslash() {
    let base = create_virtual_disk_controller_device_path();
    let full = append_file_path_device_path(&base, "EFI\\BOOT\\BOOTAA64.EFI").unwrap();
    let controller_len = core::mem::size_of::<VendorHardwareDevicePath>();

    assert_eq!(full[controller_len + 4], b'\\');
    assert_eq!(full[controller_len + 5], 0);
}
