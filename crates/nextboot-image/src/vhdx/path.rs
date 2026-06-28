use alloc::string::String;
use alloc::vec::Vec;

pub fn resolve_same_volume_parent_path(child_path: &str, parent_path: &str) -> Option<String> {
    let parent_path = parent_path.trim();
    if parent_path.is_empty() || has_windows_namespace_or_drive(parent_path) {
        return None;
    }

    let combined = if is_absolute(parent_path) {
        normalize_separators(parent_path)
    } else {
        let mut base = parent_dir(child_path)?;
        if !base.ends_with('/') {
            base.push('/');
        }
        base.push_str(&normalize_separators(parent_path));
        base
    };

    normalize_absolute_path(&combined)
}

fn has_windows_namespace_or_drive(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    path.starts_with("\\\\")
        || path.starts_with("//")
        || lower.starts_with("\\\\??\\")
        || lower.starts_with("\\\\?\\")
        || lower.starts_with("\\\\.")
        || lower.starts_with("//??/")
        || lower.starts_with("//?/")
        || lower.starts_with("//.")
        || path.as_bytes().get(1) == Some(&b':')
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\')
}

fn normalize_separators(path: &str) -> String {
    let mut out = String::new();
    for ch in path.trim().chars() {
        out.push(if ch == '\\' { '/' } else { ch });
    }
    out
}

fn parent_dir(path: &str) -> Option<String> {
    let normalized = normalize_absolute_path(path)?;
    let trimmed = normalized.trim_end_matches('/');
    let slash = trimmed.rfind('/')?;
    if slash == 0 {
        Some(String::from("/"))
    } else {
        Some(trimmed[..slash].into())
    }
}

fn normalize_absolute_path(path: &str) -> Option<String> {
    let normalized = normalize_separators(path);
    if !normalized.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }

    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    Some(out)
}
