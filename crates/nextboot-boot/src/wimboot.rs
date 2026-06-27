//! WIMBOOT command-line helpers.
//!
//! Ventoy's modified wimboot accepts virtual file entries through `vf=...`
//! arguments, plus optional runtime file callback pointers via `pfsize=` and
//! `pfread=`.  This module builds those load options without depending on UEFI
//! APIs so it can be tested independently.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use core::fmt::Write;

pub const MAX_VF_ARGUMENT_LEN: usize = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimbootVirtualFile<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

impl<'a> WimbootVirtualFile<'a> {
    pub fn new(name: &'a str, source: &'a str) -> Result<Self, WimbootCommandLineError> {
        validate_virtual_file_name(name)?;
        validate_argument_value(source)?;
        validate_virtual_file_argument_len(name, source)?;
        Ok(Self { name, source })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimbootCallbacks {
    pub file_size: usize,
    pub file_read: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimbootCommandLineError {
    NoVirtualFiles,
    EmptyValue,
    InvalidWhitespace,
    InvalidNul,
    InvalidVirtualFileName,
    VirtualFileArgumentTooLong,
}

pub fn build_wimboot_command_line(
    files: &[WimbootVirtualFile<'_>],
    callbacks: Option<WimbootCallbacks>,
    boot_index: Option<u32>,
) -> Result<String, WimbootCommandLineError> {
    if files.is_empty() {
        return Err(WimbootCommandLineError::NoVirtualFiles);
    }

    let mut command = String::from("quiet");
    if let Some(index) = boot_index.filter(|index| *index != 0) {
        let _ = write!(command, " index={}", index);
    }

    for file in files {
        validate_virtual_file_name(file.name)?;
        validate_argument_value(file.source)?;
        validate_virtual_file_argument_len(file.name, file.source)?;
        command.push(' ');
        command.push_str("vf=");
        command.push_str(file.name);
        command.push(':');
        command.push_str(file.source);
    }

    if let Some(callbacks) = callbacks {
        let _ = write!(
            command,
            " pfsize={} pfread={}",
            pointer_arg(callbacks.file_size),
            pointer_arg(callbacks.file_read)
        );
    }

    Ok(command)
}

pub fn patch_bcd_for_efi(data: &mut [u8]) -> usize {
    const SEARCH: &[u8; 4] = b".exe";
    const REPLACE: &[u8; 4] = b".efi";
    const UTF16_SEARCH_LEN: usize = SEARCH.len() * 2;

    let mut patched = 0usize;
    if data.len() < UTF16_SEARCH_LEN {
        return 0;
    }

    for offset in 0..=data.len() - UTF16_SEARCH_LEN {
        if !utf16le_ascii_eq_ignore_case(&data[offset..offset + UTF16_SEARCH_LEN], SEARCH) {
            continue;
        }

        for (index, byte) in REPLACE.iter().copied().enumerate() {
            data[offset + index * 2] = byte;
            data[offset + index * 2 + 1] = 0;
        }
        patched += 1;
    }

    patched
}

fn utf16le_ascii_eq_ignore_case(candidate: &[u8], expected: &[u8]) -> bool {
    if candidate.len() != expected.len() * 2 {
        return false;
    }

    for (index, expected) in expected.iter().copied().enumerate() {
        let low = candidate[index * 2];
        let high = candidate[index * 2 + 1];
        if high != 0 || !low.eq_ignore_ascii_case(&expected) {
            return false;
        }
    }

    true
}

fn pointer_arg(value: usize) -> String {
    format!("0x{:x}", value)
}

fn validate_virtual_file_name(value: &str) -> Result<(), WimbootCommandLineError> {
    validate_argument_value(value)?;
    if value.contains(':') || value.contains('=') {
        return Err(WimbootCommandLineError::InvalidVirtualFileName);
    }
    Ok(())
}

fn validate_argument_value(value: &str) -> Result<(), WimbootCommandLineError> {
    if value.is_empty() {
        return Err(WimbootCommandLineError::EmptyValue);
    }
    if value.bytes().any(|byte| byte == 0) {
        return Err(WimbootCommandLineError::InvalidNul);
    }
    if value.chars().any(char::is_whitespace) {
        return Err(WimbootCommandLineError::InvalidWhitespace);
    }
    Ok(())
}

fn validate_virtual_file_argument_len(
    name: &str,
    source: &str,
) -> Result<(), WimbootCommandLineError> {
    let len = name
        .len()
        .checked_add(1)
        .and_then(|len| len.checked_add(source.len()))
        .ok_or(WimbootCommandLineError::VirtualFileArgumentTooLong)?;
    if len > MAX_VF_ARGUMENT_LEN {
        return Err(WimbootCommandLineError::VirtualFileArgumentTooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn builds_wimboot_command_line_with_virtual_files_and_callbacks() {
        let files = [
            WimbootVirtualFile::new("boot.wim", "/sources/boot.wim").unwrap(),
            WimbootVirtualFile::new("bcd", "mem:0x1000:size:0x200").unwrap(),
        ];
        let callbacks = WimbootCallbacks {
            file_size: 0x1234,
            file_read: 0xabcd,
        };

        let command = build_wimboot_command_line(&files, Some(callbacks), Some(2)).unwrap();

        assert_eq!(
            command,
            "quiet index=2 vf=boot.wim:/sources/boot.wim vf=bcd:mem:0x1000:size:0x200 pfsize=0x1234 pfread=0xabcd"
        );
    }

    #[test]
    fn omits_zero_boot_index_and_callbacks() {
        let files = [WimbootVirtualFile::new("boot.wim", "/boot.wim").unwrap()];

        let command = build_wimboot_command_line(&files, None, Some(0)).unwrap();

        assert_eq!(command, "quiet vf=boot.wim:/boot.wim");
    }

    #[test]
    fn preserves_multiple_callback_backed_virtual_files() {
        let files = [
            WimbootVirtualFile::new("boot.wim", "nb-boot-wim").unwrap(),
            WimbootVirtualFile::new("vtoy_wimboot", "nb-wimboot").unwrap(),
            WimbootVirtualFile::new("bcd", "nb-bcd").unwrap(),
            WimbootVirtualFile::new("boot.sdi", "nb-boot-sdi").unwrap(),
        ];

        let command = build_wimboot_command_line(&files, None, Some(1)).unwrap();

        assert_eq!(
            command,
            "quiet index=1 vf=boot.wim:nb-boot-wim vf=vtoy_wimboot:nb-wimboot vf=bcd:nb-bcd vf=boot.sdi:nb-boot-sdi"
        );
    }

    #[test]
    fn rejects_empty_file_list_and_unsafe_tokens() {
        assert_eq!(
            build_wimboot_command_line(&[], None, None),
            Err(WimbootCommandLineError::NoVirtualFiles)
        );
        assert_eq!(
            WimbootVirtualFile::new("boot:wim", "/boot.wim"),
            Err(WimbootCommandLineError::InvalidVirtualFileName)
        );
        assert_eq!(
            WimbootVirtualFile::new("boot.wim", "/path with spaces/boot.wim"),
            Err(WimbootCommandLineError::InvalidWhitespace)
        );
        assert_eq!(
            WimbootVirtualFile::new("", "/boot.wim"),
            Err(WimbootCommandLineError::EmptyValue)
        );
    }

    #[test]
    fn rejects_virtual_file_arguments_that_wimboot_would_truncate() {
        let long_source = "/abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.wim";

        assert_eq!(
            WimbootVirtualFile::new("boot.wim", long_source),
            Err(WimbootCommandLineError::VirtualFileArgumentTooLong)
        );
    }

    #[test]
    fn patches_utf16_bcd_exe_paths_for_efi_boot() {
        let mut bcd = Vec::new();
        bcd.extend_from_slice(b"regf");
        push_utf16le(&mut bcd, "\\Windows\\System32\\bootmgr.exe");
        bcd.extend_from_slice(b".exe");
        push_utf16le(&mut bcd, "\\BOOT\\BOOTSECT.EXE");

        let patched = patch_bcd_for_efi(&mut bcd);

        assert_eq!(patched, 2);
        assert!(contains_utf16le(&bcd, "\\Windows\\System32\\bootmgr.efi"));
        assert!(contains_utf16le(&bcd, "\\BOOT\\BOOTSECT.efi"));
        assert!(bcd.windows(4).any(|window| window == b".exe"));
    }

    #[test]
    fn leaves_bcd_without_utf16_exe_paths_unchanged() {
        let mut bcd = b"regf plain ascii .exe".to_vec();
        let original = bcd.clone();

        assert_eq!(patch_bcd_for_efi(&mut bcd), 0);
        assert_eq!(bcd, original);
    }

    fn push_utf16le(out: &mut Vec<u8>, value: &str) {
        for byte in value.bytes() {
            out.push(byte);
            out.push(0);
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }

    fn contains_utf16le(data: &[u8], needle: &str) -> bool {
        let mut encoded = Vec::new();
        for byte in needle.bytes() {
            encoded.push(byte);
            encoded.push(0);
        }
        data.windows(encoded.len())
            .any(|window| window == encoded.as_slice())
    }
}
