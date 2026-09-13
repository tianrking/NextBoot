//! NextBoot - UEFI Bootloader Entry Point
//!
//! 这是 NextBoot 的主入口点，负责初始化 UEFI 服务并启动主流程。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use log::{error, info, warn};
use uefi::prelude::*;

mod boot;
mod init;
mod media_grow;
mod media_grow_util;
mod scanner;
mod source_disk;
mod ui;
mod ventoy;
mod ventoy_linux;
mod virtual_fs;
mod vlnk;
mod wim;
mod wimboot;
mod xz;

pub(crate) use nextboot_config as ventoy_config;
pub(crate) use nextboot_image::{vdi, vhdx};

use boot::BootManager;
use scanner::IsoScanner;
use ui::{format_size, show_error, show_menu, show_message, wait_for_key};

/// 应用版本
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Human-readable identifier compiled into this particular EFI binary.
///
/// A semantic package version alone is not enough to diagnose removable
/// media: multiple release candidates and local builds can legitimately be
/// `0.1.0`.  The release builder supplies a tag and developer builds supply a
/// short Git revision, so a firmware photo and serial log can identify the
/// exact binary that was started.
pub(crate) const BUILD_ID: &str = match option_env!("NEXTBOOT_BUILD_ID") {
    Some(value) => value,
    None => "unlabeled",
};

/// UEFI 入口点
#[entry]
fn efi_main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    // 初始化 UEFI 服务
    if let Err(_e) = init::uefi_services(&mut st) {
        // 无法使用日志，直接输出
        return Status::ABORTED;
    }

    info!("NextBoot v{} ({}) starting...", VERSION, BUILD_ID);
    info!("UEFI Revision: {:?}", st.uefi_revision());

    // 获取 Boot Services
    // 主启动流程
    match main_flow(image, &mut st) {
        Ok(_) => {
            info!("Boot process completed successfully");
            Status::SUCCESS
        }
        Err(e) => {
            error!("Boot failed: {:?}", e);
            // 显示错误信息给用户
            show_error(&mut st, &format!("Boot failed: {:?}", e));
            Status::ABORTED
        }
    }
}

/// 主启动流程
fn main_flow(image: Handle, st: &mut SystemTable<Boot>) -> uefi::Result<()> {
    media_grow::grow_boot_media(image, st.boot_services());
    let qemu_linux_serial_console = qemu_linux_serial_console_enabled(st);
    if qemu_linux_serial_console {
        info!("QEMU/EDK2 firmware detected; Linux serial console smoke mode enabled");
    }

    // Phase 1: 检测存储设备
    info!("Phase 1: Detecting storage devices...");
    let devices = match init::detect_storage_devices(st.boot_services()) {
        Ok(devices) => devices,
        Err(err) if err.status() == Status::NOT_FOUND => {
            warn!("No BlockIO handles found; continuing with SimpleFileSystem scan");
            Vec::new()
        }
        Err(err) => return Err(err),
    };
    info!("Found {} storage device(s)", devices.len());

    // 显示设备信息
    if devices.is_empty() {
        warn!("No physical BlockIO devices found; scanning firmware file-system volumes anyway");
    }
    for (i, device) in devices.iter().enumerate() {
        info!(
            "  [{}] {} - {} blocks, {} bytes/block, {}",
            i,
            if device.removable {
                "Removable"
            } else {
                "Fixed"
            },
            device.total_blocks,
            device.block_size,
            if init::is_4k_native(device) {
                "4K Native"
            } else {
                "512B"
            }
        );
    }

    // Phase 2: 扫描 ISO 文件
    info!("Phase 2: Scanning for ISO files across all visible data volumes...");
    let boot_device = init::get_boot_device(st.boot_services(), image)
        .ok()
        .flatten();
    let scanner = IsoScanner::from_boot_device(st.boot_services(), boot_device);
    let iso_files = scanner.scan("/")?;

    if iso_files.is_empty() {
        warn!("No ISO files found");
        show_message(st, "No ISO files found. Press any key to exit.");
        wait_for_key(st);
        return Err(uefi::Status::NOT_FOUND.into());
    }

    info!("Found {} ISO file(s)", iso_files.len());
    for (i, iso) in iso_files.iter().enumerate() {
        let wim_detail = iso
            .wim_info
            .map(|info| {
                format!(
                    " wim_boot={} compression={:?} wimboot_supported={}",
                    info.boot_index, info.compression, info.wimboot_supported
                )
            })
            .unwrap_or_default();
        let vlnk_detail = iso
            .vlnk_target_path
            .as_ref()
            .map(|target| format!(" vlnk_target={}", target))
            .unwrap_or_default();
        info!(
            "  [{}] vol{}:{} [{}] file={} virtual={}{}{}",
            i,
            iso.volume_index,
            iso.path,
            iso.image_format,
            format_size(iso.size),
            format_size(iso.virtual_size),
            wim_detail,
            vlnk_detail
        );
    }

    let mut first_display = true;
    let mut attempt = 0u64;
    loop {
        info!("Phase 3: Displaying boot menu...");
        let Some(iso) = show_menu(st, &iso_files, first_display)? else {
            info!("No ISO selected, exiting");
            return Ok(());
        };
        first_display = false;
        attempt = attempt.saturating_add(1);
        info!("Selected: {}", iso.path);
        info!("Phase 4: Booting selected ISO...");
        let (result, can_retry) = {
            let boot_manager = BootManager::new(
                st.boot_services(),
                st.runtime_services(),
                image,
                &iso,
                qemu_linux_serial_console,
            );
            let result = boot_manager.prepare_and_boot();
            (result, boot_manager.can_retry())
        };
        if !can_retry {
            error!("Firmware retained boot resources; restart before trying another image");
            show_message(st, "Firmware could not release the previous boot device.\r\n  Press any key to restart the computer safely.");
            wait_for_key(st);
            // Do not return and unload NextBoot while firmware retains our callbacks.
            st.runtime_services().reset(
                uefi::table::runtime::ResetType::COLD, Status::SUCCESS, None,
            );
        }
        info!("Boot attempt {} resources released", attempt);
        let message = match result {
            Ok(()) => format!("{} returned to NextBoot.", iso.path),
            Err(error) => {
                error!("Boot failed: {:?}", error);
                format!("Could not boot {}. Status: {:?}.", iso.path, error.status())
            }
        };
        info!("Boot attempt ended; return-to-menu is available");
        show_message(st, &format!("{}\r\n\r\n  Press any key to return to the menu, or Esc to exit.", message));
        if wait_for_key(st) == nextboot_menu::Input::Escape {
            return Ok(());
        }
        info!("Returned to boot menu; automatic boot timeout disabled");
    }
}

fn qemu_linux_serial_console_enabled(st: &SystemTable<Boot>) -> bool {
    firmware_vendor_contains_ascii(st, b"EDK II")
        || firmware_vendor_contains_ascii(st, b"OVMF")
        || firmware_vendor_contains_ascii(st, b"QEMU")
}

fn firmware_vendor_contains_ascii(st: &SystemTable<Boot>, needle: &[u8]) -> bool {
    let vendor = st.firmware_vendor().to_u16_slice();
    vendor.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(code_unit, byte)| *code_unit == u16::from(*byte))
    })
}
