//! Minimal Ventoy `ventoy.json` plugin configuration support.
//!
//! Ventoy's plugin file is a broad JSON document. NextBoot consumes the parts
//! that directly shape image discovery and the first boot menu: global control
//! file filters, image white/black lists, menu aliases, and the boot-affecting
//! plugin metadata used later by Linux initrd and Windows runtime hooks.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

mod md5;
mod model;
mod parser_core;
mod parser_entry;
mod parser_menu;
mod parser_plugins;

pub use model::*;

fn normalize_config_path(path: &str) -> String {
    let mut out = String::new();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return out;
    }
    if !trimmed.starts_with('/') && !trimmed.starts_with('\\') {
        out.push('/');
    }
    for ch in trimmed.chars() {
        out.push(if ch == '\\' { '/' } else { ch });
    }
    out
}

fn find_matching_rule<'a, T>(rules: &'a [VentoyPathRule<T>], path: &str) -> Option<&'a T> {
    for rule in rules {
        if matches!(rule.target, VentoyPathTarget::Image(_))
            && target_matches_image(&rule.target, path)
        {
            return Some(&rule.value);
        }
    }

    for rule in rules {
        if matches!(rule.target, VentoyPathTarget::Parent(_))
            && target_matches_image(&rule.target, path)
        {
            return Some(&rule.value);
        }
    }

    None
}

fn target_matches_image(target: &VentoyPathTarget, path: &str) -> bool {
    match target {
        VentoyPathTarget::Image(pattern) => path_pattern_eq(pattern, path),
        VentoyPathTarget::Parent(parent) => path_parent_matches(parent, path),
    }
}

fn path_pattern_eq(pattern: &str, path: &str) -> bool {
    let pattern = normalize_config_path(pattern);
    let path = normalize_config_path(path);
    glob_bytes_match(pattern.as_bytes(), path.as_bytes(), true)
}

fn path_parent_matches(parent: &str, path: &str) -> bool {
    let parent = normalize_config_path(parent);
    let path = normalize_config_path(path);
    let Some(path_parent) = parent_dir(&path) else {
        return false;
    };

    if parent == "/" {
        return path_parent == "/";
    }

    glob_bytes_match(parent.as_bytes(), path_parent.as_bytes(), false)
}

fn path_dir_matches(dir: &str, path: &str) -> bool {
    if is_absolute_config_path(dir) {
        return path_parent_matches(dir, path);
    }

    let path = normalize_config_path(path);
    let Some(path_parent) = parent_dir(&path) else {
        return false;
    };
    let folder = path_parent
        .rsplit('/')
        .next()
        .unwrap_or(path_parent.as_str());
    glob_bytes_match(dir.trim().as_bytes(), folder.as_bytes(), false)
}

fn parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    let slash = trimmed.rfind('/')?;
    if slash == 0 {
        Some(String::from("/"))
    } else {
        Some(trimmed[..slash].to_string())
    }
}

fn glob_bytes_match(pattern: &[u8], path: &[u8], star_matches_separator: bool) -> bool {
    let (mut p, mut s) = (0usize, 0usize);
    let mut star = None;
    let mut star_match = 0usize;

    while s < path.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            star_match = s;
        } else if p < pattern.len() && ascii_byte_eq(pattern[p], path[s]) {
            p += 1;
            s += 1;
        } else if let Some(star_pos) = star {
            if !star_matches_separator && path.get(star_match) == Some(&b'/') {
                return false;
            }
            p = star_pos + 1;
            star_match += 1;
            s = star_match;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn ascii_byte_eq(left: u8, right: u8) -> bool {
    if left.is_ascii() && right.is_ascii() {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn clean_plugin_paths(paths: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        if is_absolute_config_path(&path) && out.try_reserve_exact(1).is_ok() {
            out.push(normalize_config_path(&path));
        }
    }
    out
}

fn clean_plugin_path(path: String) -> Option<String> {
    is_absolute_config_path(&path).then(|| normalize_config_path(&path))
}

fn clean_default_image_path(path: &str) -> Option<String> {
    let path = strip_default_image_hotkey(path).trim();
    clean_plugin_path(path.to_string())
}

fn strip_default_image_hotkey(path: &str) -> &str {
    let trimmed = path.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() > 3
        && matches!(bytes[0], b'F' | b'f')
        && matches!(bytes[1], b'2'..=b'9')
        && bytes[2] == b'>'
    {
        &trimmed[3..]
    } else {
        trimmed
    }
}

fn is_absolute_config_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\')
}

fn parse_u32_text(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut value = 0u32;
    for byte in trimmed.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    Some(value)
}

fn parse_windows_uefi_resolution_lock(text: &str) -> u8 {
    match text.trim() {
        "1" => 1,
        "2" => 2,
        _ => 0,
    }
}

#[cfg(target_arch = "aarch64")]
const VENTOY_PLATFORM_SUFFIX: &str = "aa64";
#[cfg(target_arch = "x86_64")]
const VENTOY_PLATFORM_SUFFIX: &str = "uefi";
#[cfg(target_arch = "x86")]
const VENTOY_PLATFORM_SUFFIX: &str = "ia32";
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "x86")))]
const VENTOY_PLATFORM_SUFFIX: &str = "uefi";

fn is_current_platform_plugin_key(key: &str, base: &str) -> bool {
    key.strip_prefix(base)
        .and_then(|suffix| suffix.strip_prefix('_'))
        == Some(VENTOY_PLATFORM_SUFFIX)
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
}

enum VentoyTargetKind {
    Image,
    Parent,
}

#[cfg(test)]
mod tests;
