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

/// 显示菜单
fn show_menu(
    st: &mut SystemTable<Boot>,
    iso_files: &[scanner::IsoFile],
) -> uefi::Result<Option<scanner::IsoFile>> {
    use nextboot_menu::{Input, IsoType, MenuConfig, MenuItem, MenuState};

    if !authorize_boot_password(st, iso_files)? {
        return Ok(None);
    }

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
        display_menu(st, &state, iso_files, &config, active_timeout)?;

        // 等待输入
        let input = match wait_for_key_or_timeout(st, active_timeout) {
            Some(input) => input,
            None => {
                if state.selected_item().is_some() {
                    if let Some(iso) = prepare_selected_iso(st, &iso_files[state.selected])? {
                        return Ok(Some(iso));
                    }
                }
                active_timeout = None;
                continue;
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
                    if let Some(iso) = prepare_selected_iso(st, &iso_files[idx])? {
                        return Ok(Some(iso));
                    }
                }
            }
            Input::Escape => return Ok(None),
            _ => {}
        }
    }
}

fn prepare_selected_iso(
    st: &mut SystemTable<Boot>,
    iso: &scanner::IsoFile,
) -> uefi::Result<Option<scanner::IsoFile>> {
    if !authorize_iso(st, iso)? {
        return Ok(None);
    }

    let mut selected = iso.clone();
    if !configure_ventoy_plugin_choices(st, &mut selected)? {
        return Ok(None);
    }

    Ok(Some(selected))
}

fn configure_ventoy_plugin_choices(
    st: &mut SystemTable<Boot>,
    iso: &mut scanner::IsoFile,
) -> uefi::Result<bool> {
    let Some(plugin) = iso.ventoy_plugin.as_mut() else {
        return Ok(true);
    };

    if let Some(auto_install) = plugin.auto_install.as_mut() {
        if !configure_auto_install_choice(st, auto_install)? {
            return Ok(false);
        }
    }

    if let Some(persistence) = plugin.persistence.as_mut() {
        if !configure_persistence_choice(st, persistence)? {
            return Ok(false);
        }
    }

    Ok(true)
}

fn configure_auto_install_choice(
    st: &mut SystemTable<Boot>,
    auto_install: &mut ventoy_config::VentoyAutoInstall,
) -> uefi::Result<bool> {
    let count = auto_install.templates.len();
    if !should_prompt_for_plugin_choice(auto_install.autosel, auto_install.timeout, count) {
        return Ok(true);
    }

    let Some(choice) = choose_plugin_path(
        st,
        "Auto Install Script",
        "No auto install script",
        "Use",
        &auto_install.templates,
        default_plugin_choice(auto_install.autosel, count),
        plugin_choice_timeout(auto_install.timeout),
    )?
    else {
        return Ok(false);
    };

    auto_install.autosel = Some(choice);
    Ok(true)
}

fn configure_persistence_choice(
    st: &mut SystemTable<Boot>,
    persistence: &mut ventoy_config::VentoyPersistence,
) -> uefi::Result<bool> {
    let count = persistence.backends.len();
    if !should_prompt_for_plugin_choice(persistence.autosel, persistence.timeout, count) {
        return Ok(true);
    }

    let Some(choice) = choose_plugin_path(
        st,
        "Persistence Backend",
        "No persistence",
        "Use",
        &persistence.backends,
        default_plugin_choice(persistence.autosel, count),
        plugin_choice_timeout(persistence.timeout),
    )?
    else {
        return Ok(false);
    };

    persistence.autosel = Some(choice);
    Ok(true)
}

fn should_prompt_for_plugin_choice(
    autosel: Option<usize>,
    timeout: Option<u32>,
    count: usize,
) -> bool {
    count > 0 && !(valid_plugin_choice(autosel, count).is_some() && timeout.is_none())
}

fn default_plugin_choice(autosel: Option<usize>, count: usize) -> usize {
    valid_plugin_choice(autosel, count).unwrap_or(1).min(count)
}

fn valid_plugin_choice(autosel: Option<usize>, count: usize) -> Option<usize> {
    autosel.filter(|choice| *choice <= count)
}

fn plugin_choice_timeout(timeout: Option<u32>) -> Option<u64> {
    timeout.and_then(|seconds| {
        if seconds > 0 {
            Some(u64::from(seconds))
        } else {
            None
        }
    })
}

fn choose_plugin_path(
    st: &mut SystemTable<Boot>,
    title: &str,
    none_label: &str,
    use_label: &str,
    paths: &[String],
    default_choice: usize,
    timeout: Option<u64>,
) -> uefi::Result<Option<usize>> {
    use nextboot_menu::Input;

    let mut selected = default_choice.min(paths.len());
    let mut active_timeout = timeout;

    loop {
        display_plugin_choice_menu(
            st,
            title,
            none_label,
            use_label,
            paths,
            selected,
            active_timeout,
        )?;

        let Some(input) = wait_for_key_or_timeout(st, active_timeout) else {
            return Ok(Some(selected));
        };
        active_timeout = None;

        match input {
            Input::Up => {
                selected = if selected == 0 {
                    paths.len()
                } else {
                    selected - 1
                };
            }
            Input::Down => {
                let item_count = paths.len() + 1;
                selected = (selected + 1) % item_count;
            }
            Input::Home | Input::PageUp => selected = 0,
            Input::End | Input::PageDown => selected = paths.len(),
            Input::Enter => return Ok(Some(selected)),
            Input::Escape => return Ok(None),
            Input::Char(ch) => {
                if let Some(choice) = ch.to_digit(10) {
                    let choice = choice as usize;
                    if choice <= paths.len() {
                        selected = choice;
                    }
                }
            }
            _ => {}
        }
    }
}

fn display_plugin_choice_menu(
    st: &mut SystemTable<Boot>,
    title: &str,
    none_label: &str,
    use_label: &str,
    paths: &[String],
    selected: usize,
    active_timeout: Option<u64>,
) -> uefi::Result<()> {
    let stdout = st.stdout();

    stdout.reset(false)?;
    output_text(stdout, &format!("\r\n  NextBoot v{}\r\n", VERSION))?;
    output_text(stdout, "  ════════════════════════════════════════\r\n\r\n")?;
    output_text(stdout, &format!("  {}\r\n\r\n", title))?;

    let prefix = if selected == 0 { "  > " } else { "    " };
    output_text(stdout, &format!("{}{:>2}. {}\r\n", prefix, 0, none_label))?;

    for (index, path) in paths.iter().enumerate() {
        let choice = index + 1;
        let prefix = if selected == choice { "  > " } else { "    " };
        output_text(
            stdout,
            &format!(
                "{}{:>2}. {} {}\r\n",
                prefix,
                choice,
                use_label,
                truncate_chars(path, 58)
            ),
        )?;
    }

    output_text(stdout, "\r\n  ════════════════════════════════════════\r\n")?;
    if let Some(timeout) = active_timeout {
        output_text(
            stdout,
            &format!(
                "  ↑↓: Select  Enter: Continue  Esc: Back  Auto continue in {}s\r\n",
                timeout
            ),
        )?;
    } else {
        output_text(stdout, "  ↑↓: Select  Enter: Continue  Esc: Back\r\n")?;
    }

    Ok(())
}

fn authorize_boot_password(
    st: &mut SystemTable<Boot>,
    iso_files: &[scanner::IsoFile],
) -> uefi::Result<bool> {
    let Some(password) = iso_files
        .iter()
        .find_map(|iso| iso.ventoy_boot_password.as_ref())
    else {
        return Ok(true);
    };

    for attempt in 0..3 {
        output_text(st.stdout(), "\r\n  Boot menu password required\r\n")?;
        let input = read_password(st, "  Enter password: ")?;
        if password.verify(&input) {
            output_text(st.stdout(), "\r\n")?;
            return Ok(true);
        }

        if attempt < 2 {
            output_text(st.stdout(), "\r\n  Invalid password.\r\n")?;
        }
    }

    output_text(
        st.stdout(),
        "\r\n  Invalid password. Press any key to exit.",
    )?;
    let _ = wait_for_key(st);
    Ok(false)
}

fn authorize_iso(st: &mut SystemTable<Boot>, iso: &scanner::IsoFile) -> uefi::Result<bool> {
    let Some(password) = iso.ventoy_password.as_ref() else {
        return Ok(true);
    };

    output_text(
        st.stdout(),
        &format!("\r\n  Password required for {}\r\n", iso.path),
    )?;
    let input = read_password(st, "  Enter password: ")?;
    if password.verify(&input) {
        output_text(st.stdout(), "\r\n")?;
        Ok(true)
    } else {
        output_text(
            st.stdout(),
            "\r\n  Invalid password. Press any key to return to menu.",
        )?;
        let _ = wait_for_key(st);
        Ok(false)
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
    iso_files: &[scanner::IsoFile],
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
        let class_tag = iso_files
            .get(i)
            .and_then(|iso| iso.ventoy_menu_class.as_deref())
            .map(|class| format!("[{}]", truncate_chars(class, 10)))
            .unwrap_or_default();

        // 截断长文件名
        let name = if class_tag.is_empty() {
            truncate_chars(&item.label, 30)
        } else {
            truncate_chars(&item.label, 24)
        };

        output_text(
            stdout,
            &format!(
                "{}{} {:<30} {:<12} {:>10}\r\n",
                prefix, icon, name, class_tag, size
            ),
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

    if let Some(tip) = iso_files
        .get(state.selected)
        .and_then(|iso| iso.ventoy_menu_tip.as_ref())
    {
        if !tip.tip1.is_empty() {
            output_text(
                stdout,
                &format!("  Tip: {}\r\n", truncate_chars(&tip.tip1, 72)),
            )?;
        }
        if !tip.tip2.is_empty() {
            output_text(
                stdout,
                &format!("       {}\r\n", truncate_chars(&tip.tip2, 72)),
            )?;
        }
    }

    Ok(())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let len = text.chars().count();
    if len <= max_chars {
        return text.to_string();
    }

    let keep = max_chars.saturating_sub(3);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
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

fn read_password(st: &mut SystemTable<Boot>, prompt: &str) -> uefi::Result<String> {
    output_text(st.stdout(), prompt)?;

    let mut password = String::new();
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

            if let Ok(Some(key)) = st.stdin().read_key() {
                match key {
                    uefi::proto::console::text::Key::Printable(c) => {
                        let ch = char::from(c);
                        match ch {
                            '\r' | '\n' => {
                                output_text(st.stdout(), "\r\n")?;
                                return Ok(password);
                            }
                            '\x08' | '\x7f' => {
                                if !password.is_empty() {
                                    password.pop();
                                    output_text(st.stdout(), "\x08 \x08")?;
                                }
                            }
                            ch if ch >= ' ' => {
                                password.push(ch);
                                output_text(st.stdout(), "*")?;
                            }
                            _ => {}
                        }
                    }
                    uefi::proto::console::text::Key::Special(_) => {}
                }
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
