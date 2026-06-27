use alloc::format;
use alloc::string::String;
use uefi::data_types::CString16;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};

pub(super) fn open_directory(parent: &mut Directory, path: &str) -> uefi::Result<Directory> {
    let uefi_path = to_uefi_relative_path(path);
    let c_path =
        CString16::try_from(uefi_path.as_str()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let handle = parent.open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())?;
    handle
        .into_directory()
        .ok_or_else(|| uefi::Error::new(uefi::Status::NOT_FOUND, ()))
}

pub(super) fn normalize_scan_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return String::from("/");
    }

    let mut normalized = String::from("/");
    normalized.push_str(trimmed.trim_matches('/'));
    normalized
}

pub(super) fn to_uefi_relative_path(path: &str) -> String {
    let mut out = String::new();
    for (index, part) in path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        if index > 0 {
            out.push('\\');
        }
        out.push_str(part);
    }
    out
}

pub(super) fn join_display_path(parent: &str, name: &str) -> String {
    if parent == "/" || parent.is_empty() {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

pub(super) fn cstr16_to_string(name: &uefi::CStr16) -> String {
    let mut out = String::new();
    for ch in name.as_slice() {
        let c = char::from(*ch);
        if c == '\0' {
            break;
        }
        out.push(c);
    }
    out
}

pub(super) fn has_supported_extension(name: &str, extensions: &[&str]) -> bool {
    let lower = name.to_lowercase();
    extensions.iter().any(|ext| lower.ends_with(ext))
}

pub(super) fn is_hidden_tree(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | ".Trash" | ".Spotlight-V100" | ".fseventsd"
    )
}

pub(super) fn is_ventoy_plugin_tree_path(path: &str) -> bool {
    path.trim_matches('/')
        .split('/')
        .next()
        .is_some_and(|part| part.eq_ignore_ascii_case("ventoy"))
}

pub(super) fn is_default_uefi_bootloader_path(path: &str) -> bool {
    let mut parts = path.trim_matches('/').split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(filename) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if !first.eq_ignore_ascii_case("efi") || !second.eq_ignore_ascii_case("boot") {
        return false;
    }

    filename.eq_ignore_ascii_case("bootx64.efi")
        || filename.eq_ignore_ascii_case("bootaa64.efi")
        || filename.eq_ignore_ascii_case("bootia32.efi")
        || filename.eq_ignore_ascii_case("bootarm.efi")
}

pub(super) fn is_dot_underscore_file(name: &str) -> bool {
    name.starts_with("._")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_ventoy_plugin_directory_as_non_image_tree() {
        assert!(is_ventoy_plugin_tree_path("/ventoy"));
        assert!(is_ventoy_plugin_tree_path("/Ventoy/dud/dd.iso"));
        assert!(!is_ventoy_plugin_tree_path("/ISO/ventoy-linux.iso"));
        assert!(!is_ventoy_plugin_tree_path("/persistence/ventoy.dat"));
    }

    #[test]
    fn treats_default_uefi_bootloader_paths_as_non_images() {
        assert!(is_default_uefi_bootloader_path("/EFI/BOOT/BOOTX64.EFI"));
        assert!(is_default_uefi_bootloader_path("/efi/boot/bootaa64.efi"));
        assert!(!is_default_uefi_bootloader_path("/ISO/tools.efi"));
        assert!(!is_default_uefi_bootloader_path("/EFI/tools.efi"));
    }
}
