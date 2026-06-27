//! Tiny UEFI payload used by QEMU smoke tests.

#![no_std]
#![no_main]

use core::fmt::Write;
use uefi::prelude::*;

const MARKER: &str = "NEXTBOOT_SMOKE_EFI_STARTED\r\n";

#[entry]
fn efi_main(_image: Handle, mut st: SystemTable<Boot>) -> Status {
    let stdout = st.stdout();
    let _ = stdout.write_str(MARKER);
    Status::SUCCESS
}
