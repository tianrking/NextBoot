//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use uefi::table::boot::BootServices;
use crate::init::StorageDevice;

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
            "/",
            "/ISO",
            "/iso",
            "/Images",
            "/images",
            "/Boot",
            "/boot",
        ];

        for search_path in &search_paths {
            if let Ok(files) = self.scan_directory(search_path, &extensions) {
                iso_files.extend(files);
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
        let mut files = Vec::new();

        // 尝试打开目录
        // 这里简化实现，实际需要使用文件系统驱动

        // TODO: 实现实际的目录扫描
        // 1. 打开目录
        // 2. 遍历条目
        // 3. 过滤 ISO 文件
        // 4. 获取文件信息

        Ok(files)
    }

    /// 检测 ISO 文件类型
    fn detect_iso_type(&self, _path: &str) -> OsType {
        // TODO: 读取 ISO 内部文件来检测类型
        OsType::Unknown
    }
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
