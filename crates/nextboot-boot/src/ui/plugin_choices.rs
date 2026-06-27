use super::console::{output_text, wait_for_key_or_timeout};
use super::truncate_chars;
use crate::{scanner, ventoy_config, VERSION};
use alloc::format;
use alloc::string::String;
use nextboot_menu::Input;
use uefi::prelude::*;

pub(super) fn force_ventoy_memdisk_mode(iso: &mut scanner::IsoFile) {
    let plugin = iso
        .ventoy_plugin
        .get_or_insert_with(ventoy_config::VentoyImagePlugin::default);
    plugin.auto_memdisk = true;
}

pub(super) fn configure_ventoy_plugin_choices(
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
