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

/// UEFI 入口点
#[entry]
fn efi_main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    // 初始化 UEFI 服务
    if let Err(_e) = init::uefi_services(&mut st) {
        // 无法使用日志，直接输出
        return Status::ABORTED;
    }

    info!("NextBoot v{} starting...", VERSION);
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
    let scanner = IsoScanner::new(st.boot_services());
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

    // Phase 3: 显示菜单
    info!("Phase 3: Displaying boot menu...");
    let selected_iso = show_menu(st, &iso_files)?;

    match selected_iso {
        Some(iso) => {
            info!("Selected: {}", iso.path);

            // Phase 4: 启动选中的 ISO
            info!("Phase 4: Booting selected ISO...");
            let boot_manager =
                BootManager::new(st.boot_services(), st.runtime_services(), image, &iso);
            boot_manager.prepare_and_boot()?;
        }
        None => {
            info!("No ISO selected, exiting");
        }
    }

    Ok(())
}
