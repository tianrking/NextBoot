//! Tiny first-party UEFI MiniOS used by QEMU smoke tests.

#![no_std]
#![no_main]

use core::fmt::Write;
use uefi::prelude::*;

const MINIOS_MARKER: &str = "NEXTBOOT_MINIOS_STARTED\r\n";
const LEGACY_MARKER: &str = "NEXTBOOT_SMOKE_EFI_STARTED\r\n";

#[entry]
fn efi_main(_image: Handle, mut st: SystemTable<Boot>) -> Status {
    let stdout = st.stdout();
    let _ = stdout.clear();
    let _ = stdout.write_str("\r\n");
    let _ = stdout.write_str("  NextBoot MiniOS\r\n");
    let _ = stdout.write_str("  ===============================\r\n");
    let _ = stdout.write_str("  Boot protocol : UEFI StartImage\r\n");
    let _ = stdout.write_str("  Payload       : first-party MiniOS\r\n");
    let _ = stdout.write_str("  Handoff       : OK\r\n");
    let _ = stdout.write_str("  Initrd path   : provided by NextBoot LoadFile2 when present\r\n");
    let _ = stdout.write_str("\r\n");
    let _ = stdout.write_str(MINIOS_MARKER);
    let _ = stdout.write_str(LEGACY_MARKER);
    let _ = stdout.write_str("\r\n  MiniOS will return to firmware shortly.\r\n");
    st.boot_services().stall(3_000_000);
    Status::SUCCESS
}
