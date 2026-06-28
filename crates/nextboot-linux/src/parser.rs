use alloc::{
    string::{String, ToString},
    vec::Vec,
};

/// 解析 isolinux/syslinux 配置
pub fn parse_isolinux_cfg(cfg: &str) -> Option<(String, String, String)> {
    let mut kernel = None;
    let mut initrd = None;
    let mut append = String::new();

    for line in cfg.lines() {
        let Some(line) = clean_config_line(line) else {
            continue;
        };
        let tokens = config_tokens(line);
        let Some(command) = tokens.first() else {
            continue;
        };
        let mut parts = tokens[1..].iter().map(|token| token.as_str());

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
        } else if command.eq_ignore_ascii_case("append") {
            append = strip_cmdline_initrd_tokens(parts);
            if initrd.is_none() {
                initrd = extract_initrd_from_cmdline(line, &[]);
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
    let mut vars = Vec::new();

    for line in cfg.lines() {
        let Some(line) = clean_config_line(line) else {
            continue;
        };
        let tokens = config_tokens(line);
        let Some(command) = tokens.first() else {
            continue;
        };
        let mut parts = tokens[1..].iter().map(|token| token.as_str());

        if command == "}" || command.starts_with("menuentry") {
            kernel = None;
            initrd = None;
            cmdline.clear();
            continue;
        }

        if command.eq_ignore_ascii_case("set") {
            if let Some((name, value)) = parse_grub_set(&tokens[1..]) {
                set_grub_var(&mut vars, name, value);
            }
            continue;
        }

        if command.eq_ignore_ascii_case("options") {
            options = strip_cmdline_initrd_tokens_with_vars(parts, &vars);
            continue;
        }

        if command.eq_ignore_ascii_case("linux")
            || command.eq_ignore_ascii_case("linux16")
            || command.eq_ignore_ascii_case("linuxefi")
        {
            if let Some(path) = parts
                .next()
                .and_then(|token| normalize_config_path_token_with_vars(token, &vars))
            {
                kernel = Some(path);
                cmdline = strip_cmdline_initrd_tokens_with_vars(parts, &vars);
                if cmdline.is_empty() && !options.is_empty() {
                    cmdline = options.clone();
                }
            }
        } else if command.eq_ignore_ascii_case("initrd")
            || command.eq_ignore_ascii_case("initrd16")
            || command.eq_ignore_ascii_case("initrdefi")
        {
            initrd = last_normalized_config_path_with_vars(parts, &vars);
            if let Some(result) = complete_linux_config(&kernel, &initrd, &cmdline) {
                return Some(result);
            }
        } else if command.eq_ignore_ascii_case("module")
            || command.eq_ignore_ascii_case("module2")
            || command.eq_ignore_ascii_case("module2efi")
        {
            if initrd.is_none() {
                initrd = last_normalized_config_path_with_vars(parts, &vars);
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

fn config_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if quote.is_none() && ch == '#' {
            break;
        }

        if ch == '"' || ch == '\'' {
            match quote {
                Some(active) if active == ch => quote = None,
                None => quote = Some(ch),
                _ => current.push(ch),
            }
            continue;
        }

        if quote.is_none() && ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(current);
                current = String::new();
            }
            continue;
        }

        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn parse_grub_set(tokens: &[String]) -> Option<(String, String)> {
    for token in tokens {
        let (name, value) = token.split_once('=')?;
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())
        {
            return None;
        }

        let value = strip_quotes(value);
        if value.is_empty() {
            return None;
        }

        return Some((name.to_string(), value.to_string()));
    }

    None
}

fn set_grub_var(vars: &mut Vec<(String, String)>, name: String, value: String) {
    for (existing, existing_value) in vars.iter_mut() {
        if existing == &name {
            *existing_value = value;
            return;
        }
    }

    vars.push((name, value));
}

fn strip_quotes(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
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
    last_normalized_config_path_with_vars(tokens, &[])
}

fn last_normalized_config_path_with_vars<'a>(
    tokens: impl Iterator<Item = &'a str>,
    vars: &[(String, String)],
) -> Option<String> {
    let mut found = None;
    for token in tokens {
        if let Some(path) = normalize_config_path_token_with_vars(token, vars) {
            found = Some(path);
        }
    }

    found
}

fn extract_initrd_from_cmdline(cmdline: &str, vars: &[(String, String)]) -> Option<String> {
    let mut found = None;
    for token in config_tokens(cmdline) {
        let Some(value) = token.strip_prefix("initrd=") else {
            continue;
        };

        for part in value.split(',') {
            if let Some(path) = normalize_config_path_token_with_vars(part, vars) {
                found = Some(path);
            }
        }
    }

    found
}

fn strip_cmdline_initrd_tokens<'a>(tokens: impl Iterator<Item = &'a str>) -> String {
    strip_cmdline_initrd_tokens_with_vars(tokens, &[])
}

fn strip_cmdline_initrd_tokens_with_vars<'a>(
    tokens: impl Iterator<Item = &'a str>,
    vars: &[(String, String)],
) -> String {
    let mut out = String::new();
    for token in tokens {
        if token.starts_with("initrd=") {
            if extract_initrd_from_cmdline(token, vars).is_some() {
                continue;
            }
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
    normalize_config_path_token_with_vars(token, &[])
}

fn normalize_config_path_token_with_vars(token: &str, vars: &[(String, String)]) -> Option<String> {
    let mut token = token.trim();
    token = strip_quotes(token);

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

    let expanded;
    if token.contains('$') {
        expanded = expand_grub_variables(token, vars)?;
        token = expanded.as_str();
        if token.starts_with('(') {
            let (_, suffix) = token.split_once(')')?;
            token = suffix;
        }
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

fn expand_grub_variables(token: &str, vars: &[(String, String)]) -> Option<String> {
    let mut out = String::new();
    let chars: Vec<char> = token.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '$' {
            out.push(chars[index]);
            index += 1;
            continue;
        }

        index += 1;
        let start = index;
        if index < chars.len() && chars[index] == '{' {
            index += 1;
            let name_start = index;
            while index < chars.len() && chars[index] != '}' {
                index += 1;
            }
            if index >= chars.len() {
                return None;
            }
            let name: String = chars[name_start..index].iter().collect();
            index += 1;
            out.push_str(find_grub_var(vars, &name)?);
            continue;
        }

        while index < chars.len()
            && (chars[index] == '_' || chars[index] == '-' || chars[index].is_ascii_alphanumeric())
        {
            index += 1;
        }

        if index == start {
            return None;
        }

        let name: String = chars[start..index].iter().collect();
        out.push_str(find_grub_var(vars, &name)?);
    }

    Some(out)
}

fn find_grub_var<'a>(vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    for (var_name, value) in vars {
        if var_name == name {
            return Some(value.as_str());
        }
    }

    None
}

#[cfg(test)]
mod tests;
