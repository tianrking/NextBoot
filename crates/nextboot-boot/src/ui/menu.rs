use super::console::{output_text, read_password, wait_for_key, wait_for_key_or_timeout};
use super::{format_size, truncate_chars};
use crate::{scanner, ventoy_config, VERSION};
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

fn force_ventoy_memdisk_mode(iso: &mut scanner::IsoFile) {
    let plugin = iso
        .ventoy_plugin
        .get_or_insert_with(ventoy_config::VentoyImagePlugin::default);
    plugin.auto_memdisk = true;
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
