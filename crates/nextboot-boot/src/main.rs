//! NextBoot - UEFI Bootloader Entry Point
//!
//! 这是 NextBoot 的主入口点，负责初始化 UEFI 服务并启动主流程。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use log::{error, info, warn};
use uefi::prelude::*;
use uefi::proto::console::text::Output;
use uefi::table::boot::BootServices;
use uefi::ResultExt;

mod boot;
mod init;
mod scanner;
mod source_disk;
mod vdi;
mod ventoy;
mod ventoy_config;
mod ventoy_linux;
mod vhdx;
mod virtual_fs;
mod wim;
mod wimboot;
mod xz;

use boot::BootManager;
use init::StorageDevice;
use nextboot_menu::{MenuConfig, MenuState};
use scanner::IsoScanner;

/// 应用版本
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    // Phase 1: 检测存储设备
    info!("Phase 1: Detecting storage devices...");
    let devices = init::detect_storage_devices(st.boot_services())?;
    info!("Found {} storage device(s)", devices.len());

    if devices.is_empty() {
        return Err(uefi::Status::NO_MEDIA.into());
    }

    // 显示设备信息
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

    // Phase 2: 查找包含 ISO 文件的设备
    info!("Phase 2: Locating data partition...");
    let data_device = find_data_partition(st.boot_services(), &devices)?;
    info!("Data partition found on device {}", data_device);

    // Phase 3: 扫描 ISO 文件
    info!("Phase 3: Scanning for ISO files...");
    let scanner = IsoScanner::new(st.boot_services(), &devices[data_device]);
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
        info!(
            "  [{}] vol{}:{} [{}] file={} virtual={}{}",
            i,
            iso.volume_index,
            iso.path,
            iso.image_format,
            format_size(iso.size),
            format_size(iso.virtual_size),
            wim_detail
        );
    }

    // Phase 4: 显示菜单
    info!("Phase 4: Displaying boot menu...");
    let selected_iso = show_menu(st, &iso_files)?;

    match selected_iso {
        Some(iso) => {
            info!("Selected: {}", iso.path);

            // Phase 5: 启动选中的 ISO
            info!("Phase 5: Booting selected ISO...");
            let boot_manager = BootManager::new(
                st.boot_services(),
                st.runtime_services(),
                image,
                &devices[data_device],
                iso,
            );
            boot_manager.prepare_and_boot()?;
        }
        None => {
            info!("No ISO selected, exiting");
        }
    }

    Ok(())
}

/// 查找数据分区
fn find_data_partition(bt: &BootServices, devices: &[StorageDevice]) -> uefi::Result<usize> {
    // 优先选择可移动设备 (U盘)
    for (i, device) in devices.iter().enumerate() {
        if device.removable {
            return Ok(i);
        }
    }

    // 如果没有可移动设备，选择第一个设备
    if !devices.is_empty() {
        return Ok(0);
    }

    Err(uefi::Status::NOT_FOUND.into())
}

/// 显示菜单
fn show_menu<'a>(
    st: &mut SystemTable<Boot>,
    iso_files: &'a [scanner::IsoFile],
) -> uefi::Result<Option<&'a scanner::IsoFile>> {
    use nextboot_menu::{Input, IsoType, MenuConfig, MenuItem, MenuState};

    // 转换为菜单项
    let items: Vec<MenuItem> = iso_files
        .iter()
        .map(|iso| {
            let filename = iso.path.split('/').last().unwrap_or(&iso.path);
            let label = if let Some(alias) = iso.menu_alias.as_ref() {
                alias.clone()
            } else if has_duplicate_filename(iso_files, filename) {
                format!("{} [vol {}]", filename, iso.volume_index)
            } else {
                filename.to_string()
            };

            let iso_type = match iso.os_type {
                scanner::OsType::Windows => IsoType::Windows,
                scanner::OsType::Ubuntu => IsoType::Ubuntu,
                scanner::OsType::Debian => IsoType::Debian,
                scanner::OsType::Fedora => IsoType::Fedora,
                scanner::OsType::Arch => IsoType::Arch,
                scanner::OsType::Linux => IsoType::GenericLinux,
                scanner::OsType::WinPE => IsoType::WinPE,
                _ => IsoType::Unknown,
            };

            MenuItem::new(label, iso.path.clone(), iso.virtual_size, iso_type)
        })
        .collect();

    let default_selection = default_menu_selection(iso_files);
    let menu_timeout = menu_timeout_for_selection(iso_files, default_selection);
    let mut state = MenuState::new(items);
    state.selected = default_selection.min(state.items.len().saturating_sub(1));
    let config = MenuConfig {
        title: String::from("NextBoot"),
        timeout: menu_timeout.map(u64::from),
        default_selection,
        ..Default::default()
    };

    // 简化的菜单循环
    let mut active_timeout = config.timeout;
    loop {
        // 显示菜单
        display_menu(st, &state, &config, active_timeout)?;

        // 等待输入
        let input = match wait_for_key_or_timeout(st, active_timeout) {
            Some(input) => input,
            None => {
                return Ok(state.selected_item().map(|_| &iso_files[state.selected]));
            }
        };
        active_timeout = None;

        match input {
            Input::Up => state.move_up(),
            Input::Down => state.move_down(),
            Input::Enter => {
                if state.selected_item().is_some() {
                    // 查找对应的 ISO 文件
                    let idx = state.selected;
                    return Ok(Some(&iso_files[idx]));
                }
            }
            Input::Escape => return Ok(None),
            _ => {}
        }
    }
}

fn default_menu_selection(iso_files: &[scanner::IsoFile]) -> usize {
    iso_files
        .iter()
        .position(|iso| iso.ventoy_default_image)
        .unwrap_or(0)
}

fn menu_timeout_for_selection(
    iso_files: &[scanner::IsoFile],
    default_selection: usize,
) -> Option<u32> {
    iso_files
        .get(default_selection)
        .and_then(|iso| iso.ventoy_menu_timeout)
        .or_else(|| iso_files.iter().find_map(|iso| iso.ventoy_menu_timeout))
}

fn has_duplicate_filename(iso_files: &[scanner::IsoFile], filename: &str) -> bool {
    iso_files
        .iter()
        .filter(|iso| iso.path.split('/').last().unwrap_or(&iso.path) == filename)
        .nth(1)
        .is_some()
}

/// 显示菜单
fn display_menu(
    st: &mut SystemTable<Boot>,
    state: &MenuState,
    config: &MenuConfig,
    active_timeout: Option<u64>,
) -> uefi::Result<()> {
    let stdout = st.stdout();

    // 清屏
    stdout.reset(false)?;

    // 显示标题
    output_text(stdout, &format!("\r\n  {} v{}\r\n", config.title, VERSION))?;
    output_text(stdout, "  ════════════════════════════════════════\r\n\r\n")?;

    // 显示菜单项
    for (i, item) in state.items.iter().enumerate() {
        let prefix = if i == state.selected { "  > " } else { "    " };
        let icon = item.iso_type.icon();
        let size = format_size(item.size);

        // 截断长文件名
        let name = if item.label.len() > 30 {
            format!("{}...", &item.label[..27])
        } else {
            item.label.clone()
        };

        output_text(
            stdout,
            &format!("{}{} {} {:<30} {:>10}\r\n", prefix, icon, name, "", size),
        )?;
    }

    // 显示帮助
    output_text(stdout, "\r\n  ════════════════════════════════════════\r\n")?;
    if let Some(timeout) = active_timeout {
        output_text(
            stdout,
            &format!(
                "  ↑↓: Select  Enter: Boot  Esc: Exit  Auto boot in {}s\r\n",
                timeout
            ),
        )?;
    } else {
        output_text(stdout, "  ↑↓: Select  Enter: Boot  Esc: Exit\r\n")?;
    }

    Ok(())
}

/// 输出文本
fn output_text(stdout: &mut Output, text: &str) -> uefi::Result<()> {
    stdout
        .write_str(text)
        .map_err(|_| uefi::Status::DEVICE_ERROR.into())
}

/// 显示消息
fn show_message(st: &mut SystemTable<Boot>, msg: &str) {
    let stdout = st.stdout();
    let _ = stdout.reset(false);
    let _ = output_text(stdout, &format!("\r\n  {}\r\n", msg));
}

/// 显示错误
fn show_error(st: &mut SystemTable<Boot>, msg: &str) {
    let stdout = st.stdout();
    let _ = stdout.reset(false);
    let _ = output_text(stdout, &format!("\r\n  ERROR: {}\r\n", msg));
    let _ = output_text(stdout, "\r\n  Press any key to exit...\r\n");
    let _ = wait_for_key(st);
}

/// 等待按键
fn wait_for_key(st: &mut SystemTable<Boot>) -> nextboot_menu::Input {
    loop {
        if let Some(event) = st.stdin().wait_for_key_event() {
            let mut events = [event];
            if st
                .boot_services()
                .wait_for_event(&mut events)
                .discard_errdata()
                .is_err()
            {
                continue;
            }
            if let Some(input) = read_input_key(st) {
                return input;
            }
        }
    }
}

fn wait_for_key_or_timeout(
    st: &mut SystemTable<Boot>,
    timeout_seconds: Option<u64>,
) -> Option<nextboot_menu::Input> {
    use uefi::table::boot::{EventType, TimerTrigger, Tpl};

    let Some(seconds) = timeout_seconds else {
        return Some(wait_for_key(st));
    };
    if seconds == 0 {
        return None;
    }

    let Some(key_event) = st.stdin().wait_for_key_event() else {
        return Some(wait_for_key(st));
    };
    let timer_event = match unsafe {
        st.boot_services()
            .create_event(EventType::TIMER, Tpl::APPLICATION, None, None)
    } {
        Ok(event) => event,
        Err(_) => return Some(wait_for_key(st)),
    };

    let timer_ticks = seconds.saturating_mul(10_000_000);
    if st
        .boot_services()
        .set_timer(&timer_event, TimerTrigger::Relative(timer_ticks))
        .is_err()
    {
        let _ = st.boot_services().close_event(timer_event);
        return Some(wait_for_key(st));
    }

    let mut events = [key_event, timer_event];
    let signaled = st
        .boot_services()
        .wait_for_event(&mut events)
        .discard_errdata()
        .ok();
    let (input, fallback_to_key_wait) = match signaled {
        Some(0) => {
            let input = read_input_key(st);
            let fallback = input.is_none();
            (input, fallback)
        }
        Some(1) => (None, false),
        _ => (None, true),
    };
    let [_, timer_event] = events;
    let _ = st.boot_services().close_event(timer_event);
    if fallback_to_key_wait {
        Some(wait_for_key(st))
    } else {
        input
    }
}

fn read_input_key(st: &mut SystemTable<Boot>) -> Option<nextboot_menu::Input> {
    use nextboot_menu::Input;

    let key = st.stdin().read_key().ok().flatten()?;
    match key {
        uefi::proto::console::text::Key::Special(sc) => Some(Input::from_uefi_key(sc.0, None)),
        uefi::proto::console::text::Key::Printable(c) => {
            Some(Input::from_uefi_key(0, Some(char::from(c))))
        }
    }
}

/// 格式化文件大小
fn format_size(bytes: u64) -> alloc::string::String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
