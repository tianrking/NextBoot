use super::auth::{authorize_boot_password, authorize_iso};
use super::console::{output_text, wait_for_key_or_timeout};
use super::plugin_choices::{configure_ventoy_plugin_choices, force_ventoy_memdisk_mode};
use super::{format_size, truncate_chars};
use crate::{scanner, VERSION};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use log::info;
use nextboot_menu::{Input, IsoType, MenuConfig, MenuItem, MenuState};
use uefi::prelude::*;

pub(crate) fn show_menu(
    st: &mut SystemTable<Boot>,
    iso_files: &[scanner::IsoFile],
) -> uefi::Result<Option<scanner::IsoFile>> {
    if !authorize_boot_password(st, iso_files)? {
        return Ok(None);
    }

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

    let mut active_timeout = config.timeout;
    loop {
        display_menu(st, &state, iso_files, &config, active_timeout)?;

        let input = match wait_for_key_or_timeout(st, active_timeout) {
            Some(input) => input,
            None => {
                if state.selected_item().is_some() {
                    if let Some(iso) = prepare_selected_iso(st, &iso_files[state.selected], false)?
                    {
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
                    let idx = state.selected;
                    if let Some(iso) = prepare_selected_iso(st, &iso_files[idx], false)? {
                        return Ok(Some(iso));
                    }
                }
            }
            Input::Char('m') | Input::Char('M') => {
                if state.selected_item().is_some() {
                    let idx = state.selected;
                    if let Some(iso) = prepare_selected_iso(st, &iso_files[idx], true)? {
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
    force_memdisk: bool,
) -> uefi::Result<Option<scanner::IsoFile>> {
    if !authorize_iso(st, iso)? {
        return Ok(None);
    }

    let mut selected = iso.clone();
    if !configure_ventoy_plugin_choices(st, &mut selected)? {
        return Ok(None);
    }
    if force_memdisk {
        force_ventoy_memdisk_mode(&mut selected);
        info!("Manual Ventoy memdisk mode requested for {}", selected.path);
    }

    Ok(Some(selected))
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

fn display_menu(
    st: &mut SystemTable<Boot>,
    state: &MenuState,
    iso_files: &[scanner::IsoFile],
    config: &MenuConfig,
    active_timeout: Option<u64>,
) -> uefi::Result<()> {
    let stdout = st.stdout();

    stdout.reset(false)?;
    output_text(stdout, &format!("\r\n  {} v{}\r\n", config.title, VERSION))?;
    output_text(stdout, "  ════════════════════════════════════════\r\n\r\n")?;

    for (i, item) in state.items.iter().enumerate() {
        let prefix = if i == state.selected { "  > " } else { "    " };
        let icon = item.iso_type.icon();
        let size = format_size(item.size);
        let class_tag = iso_files
            .get(i)
            .and_then(|iso| iso.ventoy_menu_class.as_deref())
            .map(|class| format!("[{}]", truncate_chars(class, 10)))
            .unwrap_or_default();
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

    output_text(stdout, "\r\n  ════════════════════════════════════════\r\n")?;
    if let Some(timeout) = active_timeout {
        output_text(
            stdout,
            &format!(
                "  ↑↓: Select  Enter: Boot  M: Memdisk  Esc: Exit  Auto boot in {}s\r\n",
                timeout
            ),
        )?;
    } else {
        output_text(
            stdout,
            "  ↑↓: Select  Enter: Boot  M: Memdisk  Esc: Exit\r\n",
        )?;
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
