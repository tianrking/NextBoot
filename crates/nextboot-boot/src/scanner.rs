//! ISO 文件扫描模块
//!
//! 负责扫描存储设备上的 ISO 文件

use crate::init::StorageDevice;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::NonNull;
use nextboot_fs::exfat::ExFat;
use nextboot_fs::fat32::Fat32;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileExtent, FileSystem, FileSystemType, FsError};
use uefi::data_types::CString16;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{BootServices, SearchType};
use uefi::{Handle, Identify};

const MAX_SCAN_DEPTH: usize = 4;

/// ISO 文件信息
#[derive(Debug, Clone)]
pub struct IsoFile {
    /// 文件路径
    pub path: String,
    /// 文件大小 (字节)
    pub size: u64,
    /// 文件所在的 UEFI volume handle
    pub volume_handle: Handle,
    /// 文件所在卷的逻辑块大小
    pub block_size: u32,
    /// 起始 LBA
    pub start_lba: u64,
    /// 文件到底层卷 BlockIO 的 extent 映射
    pub extents: Vec<IsoExtent>,
    /// 检测到的操作系统类型
    pub os_type: OsType,
}

/// ISO 文件在所在卷上的物理区段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoExtent {
    pub virtual_block_start: u64,
    pub physical_lba: u64,
    pub block_count: u64,
}

impl From<FileExtent> for IsoExtent {
    fn from(extent: FileExtent) -> Self {
        Self {
            virtual_block_start: extent.virtual_block_start,
            physical_lba: extent.physical_lba,
            block_count: extent.block_count,
        }
    }
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
            root, "/", "/ISO", "/iso", "/Images", "/images", "/Boot", "/boot",
        ];

        let fs_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;

        for handle in fs_handles.iter().copied() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };

            for search_path in &search_paths {
                if let Ok(files) = self.scan_volume_path(handle, &mut fs, search_path, &extensions)
                {
                    iso_files.extend(files);
                }
            }
        }

        // 去重 (基于路径)
        iso_files.sort_by(|a, b| a.path.cmp(&b.path));
        iso_files.dedup_by(|a, b| a.path == b.path);

        // 按名称排序
        iso_files.sort_by(|a, b| {
            a.path
                .split('/')
                .last()
                .unwrap_or(&a.path)
                .cmp(b.path.split('/').last().unwrap_or(&b.path))
        });

        Ok(iso_files)
    }

    /// 扫描单个目录
    fn scan_directory(&self, path: &str, extensions: &[&str]) -> uefi::Result<Vec<IsoFile>> {
        let fs_handles = self
            .bt
            .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
        let mut files = Vec::new();

        for handle in fs_handles.iter().copied() {
            let mut fs = match self.bt.open_protocol_exclusive::<SimpleFileSystem>(handle) {
                Ok(fs) => fs,
                Err(_) => continue,
            };

            if let Ok(mut volume_files) = self.scan_volume_path(handle, &mut fs, path, extensions) {
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
        volume_handle: Handle,
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
        self.scan_directory_entries(
            volume_handle,
            &mut dir,
            &normalized,
            extensions,
            0,
            &mut files,
        )?;
        Ok(files)
    }

    fn scan_directory_entries(
        &self,
        volume_handle: Handle,
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
                        volume_handle,
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
                let (block_size, extents) = self
                    .resolve_file_extents(volume_handle, &full_path)
                    .unwrap_or((self.device.block_size, Vec::new()));
                let start_lba = extents.first().map_or(0, |extent| extent.physical_lba);

                files.push(IsoFile {
                    path: full_path.clone(),
                    size: entry.file_size(),
                    volume_handle,
                    block_size,
                    start_lba,
                    extents,
                    os_type: self.detect_iso_type(&full_path),
                });
            }
        }

        Ok(())
    }

    fn resolve_file_extents(
        &self,
        volume_handle: Handle,
        path: &str,
    ) -> Option<(u32, Vec<IsoExtent>)> {
        let block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(volume_handle)
            .ok()?;
        let media = block_io.media();
        if !media.is_media_present() {
            return None;
        }

        let block_size = media.block_size();
        let uefi_io = UefiBlockIo::new(&block_io)?;
        let shared: nextboot_fs::SharedBlockIo = Rc::new(uefi_io);
        let mut boot_sector = alloc::vec![0u8; block_size as usize];
        shared.read_blocks(0, &mut boot_sector).ok()?;

        let extents = match detect_fs_type(&boot_sector) {
            FileSystemType::Fat32 => Fat32::open(shared)
                .and_then(|fs| fs.file_extents(path))
                .ok()?,
            FileSystemType::ExFat => ExFat::open(shared)
                .and_then(|fs| fs.file_extents(path))
                .ok()?,
            _ => return None,
        };

        Some((
            block_size,
            extents.into_iter().map(IsoExtent::from).collect(),
        ))
    }
}

struct UefiBlockIo {
    block_io: NonNull<BlockIO>,
    media_id: u32,
    block_size: u32,
    total_blocks: u64,
}

impl UefiBlockIo {
    fn new(block_io: &BlockIO) -> Option<Self> {
        let media = block_io.media();
        let block_size = media.block_size();
        if block_size == 0 {
            return None;
        }

        Some(Self {
            block_io: NonNull::from(block_io),
            media_id: media.media_id(),
            block_size,
            total_blocks: media.last_block() + 1,
        })
    }
}

impl BlockIoOps for UefiBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(FsError::InvalidArgument);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(FsError::ReadError);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| FsError::ReadError)
    }
}

fn open_directory(parent: &mut Directory, path: &str) -> uefi::Result<Directory> {
    let uefi_path = to_uefi_relative_path(path);
    let c_path =
        CString16::try_from(uefi_path.as_str()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
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
