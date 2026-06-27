use alloc::string::{String, ToString};

/// 解析 isolinux/syslinux 配置
pub fn parse_isolinux_cfg(cfg: &str) -> Option<(String, String, String)> {
    let mut kernel = None;
    let mut initrd = None;
    let mut append = String::new();

    for line in cfg.lines() {
        let Some(line) = clean_config_line(line) else {
            continue;
        };
        let line_lower = line.to_lowercase();
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            continue;
        };

        if command.eq_ignore_ascii_case("label") {
            kernel = None;
            initrd = None;
            append.clear();
            continue;
        }

        if command.eq_ignore_ascii_case("kernel") || command.eq_ignore_ascii_case("linux") {
            kernel = parts.next().and_then(normalize_config_path_token);
            if let Some(result) = complete_linux_config(&kernel, &initrd, &append) {
                return Some(result);
            }
        } else if command.eq_ignore_ascii_case("initrd") {
            initrd = last_normalized_config_path(parts);
            if let Some(result) = complete_linux_config(&kernel, &initrd, &append) {
                return Some(result);
            }
        } else if command.eq_ignore_ascii_case("append") || line_lower.starts_with("append ") {
            append = strip_cmdline_initrd_tokens(parts);
            if initrd.is_none() {
                initrd = extract_initrd_from_cmdline(line);
            }
            if let Some(result) = complete_linux_config(&kernel, &initrd, &append) {
                return Some(result);
            }
        }
    }

    complete_linux_config(&kernel, &initrd, &append)
}

/// 解析 GRUB 配置
pub fn parse_grub_cfg(cfg: &str) -> Option<(String, String, String)> {
    let mut kernel = None;
    let mut initrd = None;
    let mut cmdline = String::new();
    let mut options = String::new();

    for line in cfg.lines() {
        let Some(line) = clean_config_line(line) else {
            continue;
        };
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            continue;
        };

        if command == "}" || command.starts_with("menuentry") {
            kernel = None;
            initrd = None;
            cmdline.clear();
            continue;
        }

        if command.eq_ignore_ascii_case("options") {
            options = strip_cmdline_initrd_tokens(parts);
            continue;
        }

        if command.eq_ignore_ascii_case("linux")
            || command.eq_ignore_ascii_case("linux16")
            || command.eq_ignore_ascii_case("linuxefi")
        {
            if let Some(path) = parts.next().and_then(normalize_config_path_token) {
                kernel = Some(path);
                cmdline = strip_cmdline_initrd_tokens(parts);
                if cmdline.is_empty() && !options.is_empty() {
                    cmdline = options.clone();
                }
            }
        } else if command.eq_ignore_ascii_case("initrd")
            || command.eq_ignore_ascii_case("initrd16")
            || command.eq_ignore_ascii_case("initrdefi")
        {
            initrd = last_normalized_config_path(parts);
            if let Some(result) = complete_linux_config(&kernel, &initrd, &cmdline) {
                return Some(result);
            }
        }
    }

    complete_linux_config(&kernel, &initrd, &cmdline)
}

fn clean_config_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    Some(trimmed)
}

fn complete_linux_config(
    kernel: &Option<String>,
    initrd: &Option<String>,
    cmdline: &str,
) -> Option<(String, String, String)> {
    match (kernel, initrd) {
        (Some(kernel), Some(initrd)) => Some((kernel.clone(), initrd.clone(), cmdline.to_string())),
        _ => None,
    }
}

fn last_normalized_config_path<'a>(tokens: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut found = None;
    for token in tokens {
        if let Some(path) = normalize_config_path_token(token) {
            found = Some(path);
        }
    }

    found
}

fn extract_initrd_from_cmdline(cmdline: &str) -> Option<String> {
    let mut found = None;
    for token in cmdline.split_whitespace() {
        let Some(value) = token.strip_prefix("initrd=") else {
            continue;
        };

        for part in value.split(',') {
            if let Some(path) = normalize_config_path_token(part) {
                found = Some(path);
            }
        }
    }

    found
}

fn strip_cmdline_initrd_tokens<'a>(tokens: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for token in tokens {
        if token.starts_with("initrd=") {
            continue;
        }

        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }

    out
}

fn normalize_config_path_token(token: &str) -> Option<String> {
    let mut token = token.trim();
    token = token.trim_matches('"').trim_matches('\'');

    if let Some(value) = token.strip_prefix("initrd=") {
        token = value;
    }

    if let Some((first, _)) = token.split_once(',') {
        token = first;
    }

    if token.starts_with('(') {
        let (_, suffix) = token.split_once(')')?;
        token = suffix;
    }

    while let Some(suffix) = token.strip_prefix("./") {
        token = suffix;
    }

    let token = token.trim();
    if token.is_empty()
        || token.contains('$')
        || token.contains("://")
        || token.eq_ignore_ascii_case("none")
    {
        return None;
    }

    let mut path = String::new();
    for ch in token.chars() {
        path.push(if ch == '\\' { '/' } else { ch });
    }

    Some(path)
}

#[cfg(test)]
mod tests {
    use super::{parse_grub_cfg, parse_isolinux_cfg};
    use alloc::string::String;

    #[test]
    fn grub_parser_extracts_first_menuentry_paths() {
        let cfg = r#"
            menuentry 'Try Ubuntu' {
                linuxefi (loop)/casper/vmlinuz boot=casper quiet splash ---
                initrdefi (loop)/casper/initrd
            }
        "#;

        assert_eq!(
            parse_grub_cfg(cfg),
            Some((
                String::from("/casper/vmlinuz"),
                String::from("/casper/initrd"),
                String::from("boot=casper quiet splash ---")
            ))
        );
    }

    #[test]
    fn grub_parser_uses_last_initrd_component() {
        let cfg = r#"
            linux /arch/boot/x86_64/vmlinuz-linux archisobasedir=arch
            initrd /intel-ucode.img /arch/boot/x86_64/initramfs-linux.img
        "#;

        assert_eq!(
            parse_grub_cfg(cfg),
            Some((
                String::from("/arch/boot/x86_64/vmlinuz-linux"),
                String::from("/arch/boot/x86_64/initramfs-linux.img"),
                String::from("archisobasedir=arch")
            ))
        );
    }

    #[test]
    fn grub_parser_uses_bls_options_line() {
        let cfg = r#"
            title Fedora Live
            options root=live:CDLABEL=Fedora quiet rhgb
            linux /images/pxeboot/vmlinuz
            initrd /images/pxeboot/initrd.img
        "#;

        assert_eq!(
            parse_grub_cfg(cfg),
            Some((
                String::from("/images/pxeboot/vmlinuz"),
                String::from("/images/pxeboot/initrd.img"),
                String::from("root=live:CDLABEL=Fedora quiet rhgb")
            ))
        );
    }

    #[test]
    fn isolinux_parser_extracts_append_initrd_and_removes_duplicate_arg() {
        let cfg = r#"
            label live
              kernel vmlinuz
              append initrd=initrd.img boot=live quiet
        "#;

        assert_eq!(
            parse_isolinux_cfg(cfg),
            Some((
                String::from("vmlinuz"),
                String::from("initrd.img"),
                String::from("boot=live quiet")
            ))
        );
    }
}
