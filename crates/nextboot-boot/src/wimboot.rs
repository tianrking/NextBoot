//! WIMBOOT command-line helpers.
//!
//! Ventoy's modified wimboot accepts virtual file entries through `vf=...`
//! arguments, plus optional runtime file callback pointers via `pfsize=` and
//! `pfread=`.  This module builds those load options without depending on UEFI
//! APIs so it can be tested independently.

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
}
