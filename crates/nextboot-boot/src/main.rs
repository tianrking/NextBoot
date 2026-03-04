//! NextBoot - UEFI Bootloader Entry Point
//!
//! 这是 NextBoot 的主入口点，负责初始化 UEFI 服务并启动主流程。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use log::{info, error};
use uefi::prelude::*;
use uefi::table::boot::BootServices;

mod init;

/// UEFI 入口点
#[entry]
fn efi_main(image: Handle, st: SystemTable<Boot>) -> Status {
    // 初始化 UEFI 服务
    if let Err(e) = init::uefi_services(&image, &st) {
        error!("Failed to initialize UEFI services: {:?}", e);
        return Status::ABORTED;
    }

    info!("NextBoot v{} starting...", env!("CARGO_PKG_VERSION"));

    // 获取 Boot Services
    let bt = st.boot_services();

    // 主启动流程
    match main_flow(bt) {
        Ok(_) => {
            info!("Boot process completed successfully");
            Status::SUCCESS
        }
        Err(e) => {
            error!("Boot failed: {:?}", e);
            // 显示错误信息给用户
            st.stdout().reset(false).unwrap();
            st.stdout()
                .output_string(s!("Boot failed. Press any key to exit."))
                .unwrap();
            bt.wait_for_key_event(st.stdin()).unwrap();
            Status::ABORTED
        }
    }
}

/// 主启动流程
fn main_flow(bt: &BootServices) -> uefi::Result<()> {
    // Phase 1: 检测存储设备
    info!("Phase 1: Detecting storage devices...");
    let devices = init::detect_storage_devices(bt)?;
    info!("Found {} storage device(s)", devices.len());

    // Phase 2: 查找 Data 分区 (exFAT)
    info!("Phase 2: Locating data partition...");
    // TODO: 实现分区检测

    // Phase 3: 扫描 ISO 文件
    info!("Phase 3: Scanning for ISO files...");
    // TODO: 实现 ISO 扫描

    // Phase 4: 显示菜单
    info!("Phase 4: Displaying boot menu...");
    // TODO: 实现菜单显示

    // Phase 5: 启动选中的 ISO
    info!("Phase 5: Booting selected ISO...");
    // TODO: 实现引导启动

    Ok(())
}

/// 全局 panic 处理
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    error!("PANIC: {}", info);
    loop {
        // UEFI 环境下的无限循环
        core::hint::spin_loop();
    }
}
