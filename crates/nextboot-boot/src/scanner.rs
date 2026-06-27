//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use uefi::data_types::CString16;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::Identify;
use crate::init::StorageDevice;

const MAX_SCAN_DEPTH: usize = 4;

/// ISO 文件信息
#[derive(Debug, Clone)]
pub struct IsoFile {
    /// 文件路径
    pub path: String,
    /// 文件大小 (字节)
    pub size: u64,
    /// 起始 LBA
    pub start_lba: u64,
    /// 检测到的操作系统类型
    pub os_type: OsType,
}

/// 操作系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Windows,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    Linux,
    WinPE,
    Unknown,
}

impl OsType {
    /// 从文件名检测
    pub fn detect_from_path(path: &str) -> Self {
        let path_lower = path.to_lowercase();

        if path_lower.contains("windows") {
            return OsType::Windows;
        }
        if path_lower.contains("ubuntu") {
            return OsType::Ubuntu;
        }
        if path_lower.contains("debian") {
            return OsType::Debian;
        }
        if path_lower.contains("fedora") {
            return OsType::Fedora;
        }
        if path_lower.contains("arch") || path_lower.contains("manjaro") {
            return OsType::Arch;
        }
        if path_lower.contains("winpe") || path_lower.contains("pe_") {
            return OsType::WinPE;
        }
        if path_lower.contains("linux") {
            return OsType::Linux;
        }

        OsType::Unknown
    }
}

/// ISO 扫描器
pub struct IsoScanner<'a> {
    bt: &'a BootServices,
    device: &'a StorageDevice,
}

impl<'a> IsoScanner<'a> {
    /// 创建新的扫描器
    pub fn new(bt: &'a BootServices, device: &'a StorageDevice) -> Self {
        Self { bt, device }
    }

    /// 扫描指定目录下的 ISO 文件
    pub fn scan(&self, root: &str) -> uefi::Result<Vec<IsoFile>> {
        let mut iso_files = Vec::new();

        // 支持的文件扩展名
        let extensions = [".iso", ".wim", ".img", ".vhd", ".esd"];

        // 扫描常见目录
        let search_paths = [
            root,
            "/",
            "/ISO",
            "/iso",
            "/Images",
            "/images",
            "/Boot",
            "/boot",
        ];

        let fs_handles = self.bt.locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;

        for handle in fs_handles.iter().copied() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };

            for search_path in &search_paths {
                if let Ok(files) = self.scan_volume_path(&mut fs, search_path, &extensions) {
                    iso_files.extend(files);
                }
            }
        }

        // 去重 (基于路径)
        iso_files.sort_by(|a, b| a.path.cmp(&b.path));
        iso_files.dedup_by(|a, b| a.path == b.path);

        // 按名称排序
        iso_files.sort_by(|a, b| {
            a.path.split('/')
                .last()
                .unwrap_or(&a.path)
                .cmp(b.path.split('/').last().unwrap_or(&b.path))
        });

        Ok(iso_files)
    }

    /// 扫描单个目录
    fn scan_directory(&self, path: &str, extensions: &[&str]) -> uefi::Result<Vec<IsoFile>> {
        let fs_handles = self.bt.locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
        let mut files = Vec::new();

        for handle in fs_handles.iter().copied() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };

            if let Ok(mut volume_files) = self.scan_volume_path(&mut fs, path, extensions) {
                files.append(&mut volume_files);
            }
        }

        Ok(files)
    }

    /// 检测 ISO 文件类型
    fn detect_iso_type(&self, path: &str) -> OsType {
        OsType::detect_from_path(path)
    }

    fn scan_volume_path(
        &self,
        fs: &mut SimpleFileSystem,
        path: &str,
        extensions: &[&str],
    ) -> uefi::Result<Vec<IsoFile>> {
        let mut root = fs.open_volume()?;
        let normalized = normalize_scan_path(path);
        let mut dir = if normalized == "/" {
            root
        } else {
            match open_directory(&mut root, &normalized) {
                Ok(dir) => dir,
                Err(e) => return Err(e),
            }
        };

        let mut files = Vec::new();
        self.scan_directory_entries(&mut dir, &normalized, extensions, 0, &mut files)?;
        Ok(files)
    }

    fn scan_directory_entries(
        &self,
        dir: &mut Directory,
        display_path: &str,
        extensions: &[&str],
        depth: usize,
        files: &mut Vec<IsoFile>,
    ) -> uefi::Result<()> {
        while let Some(entry) = dir.read_entry_boxed()? {
            let name = cstr16_to_string(entry.file_name());

            if name.is_empty() || name == "." || name == ".." {
                continue;
            }

            let full_path = join_display_path(display_path, &name);

            if entry.is_directory() {
                if depth >= MAX_SCAN_DEPTH || is_hidden_tree(&name) {
                    continue;
                }

                if let Ok(mut child) = open_directory(dir, &name) {
                    let _ = self.scan_directory_entries(
                        &mut child,
                        &full_path,
                        extensions,
                        depth + 1,
                        files,
                    );
                }
                continue;
            }

            if has_supported_extension(&name, extensions) {
                files.push(IsoFile {
                    path: full_path.clone(),
                    size: entry.file_size(),
                    // UEFI SimpleFileSystem exposes file paths but not physical extents.
                    // The raw Block IO mapper will fill this when Ventoy-style virtual media is wired in.
                    start_lba: 0,
                    os_type: self.detect_iso_type(&full_path),
                });
            }
        }

        Ok(())
    }
}

fn open_directory(parent: &mut Directory, path: &str) -> uefi::Result<Directory> {
    let uefi_path = to_uefi_relative_path(path);
    let c_path = CString16::try_from(uefi_path.as_str())
        .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let handle = parent.open(c_path.as_ref(), FileMode::Read, FileAttribute::empty())?;
    handle
        .into_directory()
        .ok_or_else(|| uefi::Error::new(uefi::Status::NOT_FOUND, ()))
}

fn normalize_scan_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return String::from("/");
    }

    let mut normalized = String::from("/");
    normalized.push_str(trimmed.trim_matches('/'));
    normalized
}

fn to_uefi_relative_path(path: &str) -> String {
    let mut out = String::new();
    for (index, part) in path.trim_matches('/').split('/').filter(|s| !s.is_empty()).enumerate() {
        if index > 0 {
            out.push('\\');
        }
        out.push_str(part);
    }
    out
}

fn join_display_path(parent: &str, name: &str) -> String {
    if parent == "/" || parent.is_empty() {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn cstr16_to_string(name: &uefi::CStr16) -> String {
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

fn has_supported_extension(name: &str, extensions: &[&str]) -> bool {
    let lower = name.to_lowercase();
    extensions.iter().any(|ext| lower.ends_with(ext))
}

fn is_hidden_tree(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | ".Trash" | ".Spotlight-V100" | ".fseventsd"
    )
}

/// 缓存的 ISO 列表
pub struct IsoCache {
    entries: Vec<IsoFile>,
    timestamp: u64,
}

impl IsoCache {
    /// 创建新缓存
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            timestamp: 0,
        }
    }

    /// 从缓存加载
    pub fn load(&self) -> Option<&[IsoFile]> {
        if self.entries.is_empty() {
            None
        } else {
            Some(&self.entries)
        }
    }

    /// 保存到缓存
    pub fn save(&mut self, entries: Vec<IsoFile>) {
        self.entries = entries;
        // timestamp = current_time
    }

    /// 清除缓存
    pub fn clear(&mut self) {
        self.entries.clear();
        self.timestamp = 0;
    }

    /// 检查缓存是否有效
    pub fn is_valid(&self, max_age_seconds: u64) -> bool {
        // TODO: 检查时间戳
        !self.entries.is_empty()
    }
}

impl Default for IsoCache {
    fn default() -> Self {
        Self::new()
    }
}
