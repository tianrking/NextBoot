//! Read-only Simple File System protocol backed by the selected ISO.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::udf::Udf;
use nextboot_fs::{
    FileAttributes as FsFileAttributes, FileExtent, FileInfo as FsFileInfo, FileSystem, FsError,
};
use uefi::proto::unsafe_protocol;
use uefi::table::boot::BootServices;
use uefi::{Guid, Handle, Identify, Status};

mod file_protocol;
mod info;

use file_protocol::IsoFileProtocol;

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("964e5b22-6459-11d2-8e39-00a0c969723b")]
pub struct IsoSimpleFileSystem(SimpleFileSystemProtocol);

#[derive(Debug)]
#[repr(C)]
pub struct SimpleFileSystemProtocol {
    revision: u64,
    open_volume: unsafe extern "efiapi" fn(
        this: *mut SimpleFileSystemProtocol,
        root: *mut *mut FileProtocol,
    ) -> Status,
}

#[derive(Debug)]
#[repr(C)]
pub struct FileProtocol {
    revision: u64,
    open: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
    ) -> Status,
    close: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    delete: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    read: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    write: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        buffer_size: *mut usize,
        buffer: *const c_void,
    ) -> Status,
    get_position:
        unsafe extern "efiapi" fn(this: *const FileProtocol, position: *mut u64) -> Status,
    set_position: unsafe extern "efiapi" fn(this: *mut FileProtocol, position: u64) -> Status,
    get_info: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        information_type: *const Guid,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    set_info: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        information_type: *const Guid,
        buffer_size: usize,
        buffer: *const c_void,
    ) -> Status,
    flush: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    open_ex: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
        token: *mut FileIoToken,
    ) -> Status,
    read_ex: unsafe extern "efiapi" fn(this: *mut FileProtocol, token: *mut FileIoToken) -> Status,
    write_ex: unsafe extern "efiapi" fn(this: *mut FileProtocol, token: *mut FileIoToken) -> Status,
    flush_ex: unsafe extern "efiapi" fn(this: *mut FileProtocol, token: *mut FileIoToken) -> Status,
}

#[derive(Debug)]
#[repr(C)]
struct FileIoToken {
    event: *mut c_void,
    status: Status,
    buffer_size: usize,
    buffer: *mut c_void,
}

const SIMPLE_FILE_SYSTEM_REVISION: u64 = 0x0001_0000;
const FILE_PROTOCOL_REVISION: u64 = 0x0002_0000;

const EFI_FILE_MODE_READ: u64 = 0x0000_0000_0000_0001;
const EFI_FILE_MODE_WRITE: u64 = 0x0000_0000_0000_0002;
const EFI_FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;

const EFI_FILE_ATTR_READ_ONLY: u64 = 0x0000_0000_0000_0001;
const EFI_FILE_ATTR_HIDDEN: u64 = 0x0000_0000_0000_0002;
const EFI_FILE_ATTR_SYSTEM: u64 = 0x0000_0000_0000_0004;
const EFI_FILE_ATTR_DIRECTORY: u64 = 0x0000_0000_0000_0010;
const EFI_FILE_ATTR_ARCHIVE: u64 = 0x0000_0000_0000_0020;

const EFI_FILE_INFO_GUID: Guid = uefi::guid!("09576e92-6d3f-11d2-8e39-00a0c969723b");
const EFI_FILE_SYSTEM_INFO_GUID: Guid = uefi::guid!("09576e93-6d3f-11d2-8e39-00a0c969723b");
const EFI_FILE_SYSTEM_VOLUME_LABEL_GUID: Guid = uefi::guid!("db47d7d3-fe81-11d3-9a35-0090273fc14d");

const EFI_FILE_INFO_NAME_OFFSET: usize = 80;
const EFI_FILE_SYSTEM_INFO_LABEL_OFFSET: usize = 36;
const VOLUME_LABEL: &str = "NEXTBOOT";

pub enum VirtualIsoFilesystem {
    Iso9660(Iso9660),
    Udf(Udf),
}

impl VirtualIsoFilesystem {
    pub fn block_size(&self) -> u32 {
        match self {
            Self::Iso9660(fs) => fs.block_size(),
            Self::Udf(fs) => fs.block_size(),
        }
    }

    pub fn read_dir(&self, path: &str) -> Result<Vec<FsFileInfo>, FsError> {
        match self {
            Self::Iso9660(fs) => fs.read_dir(path),
            Self::Udf(fs) => fs.read_dir(path),
        }
    }

    pub fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        match self {
            Self::Iso9660(fs) => fs.read_file(path, offset, buf),
            Self::Udf(fs) => fs.read_file(path, offset, buf),
        }
    }

    pub fn stat(&self, path: &str) -> Result<FsFileInfo, FsError> {
        match self {
            Self::Iso9660(fs) => fs.stat(path),
            Self::Udf(fs) => fs.stat(path),
        }
    }

    pub fn file_extents(&self, path: &str) -> Result<Vec<FileExtent>, FsError> {
        match self {
            Self::Iso9660(fs) => fs.file_extents(path),
            Self::Udf(fs) => fs.file_extents(path),
        }
    }
}

pub struct VirtualFileReplacement {
    path: String,
    data: Vec<u8>,
}

impl VirtualFileReplacement {
    pub fn new(path: &str, data: Vec<u8>) -> Self {
        Self {
            path: normalize_path_segments(path),
            data,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

struct FileReplacementSet {
    entries: Vec<FileReplacementEntry>,
}

impl FileReplacementSet {
    fn new(replacements: Vec<VirtualFileReplacement>) -> Self {
        let entries = replacements
            .into_iter()
            .map(FileReplacementEntry::new)
            .collect();
        Self { entries }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn find_index(&self, path: &str) -> Option<usize> {
        let key = path_key(path);
        self.entries.iter().position(|entry| entry.key == key)
    }

    fn data(&self, index: usize) -> Option<&[u8]> {
        self.entries
            .get(index)
            .map(|entry| entry.replacement.data.as_slice())
    }

    fn apply_to_file_info(&self, index: usize, info: &mut FsFileInfo) {
        let Some(data) = self.data(index) else {
            return;
        };
        info.size = data.len() as u64;
        info.is_dir = false;
        info.start_cluster = 0;
        info.contiguous = false;
        info.attributes.remove(FsFileAttributes::DIRECTORY);
    }

    fn apply_to_directory(&self, dir: &str, entries: &mut [FsFileInfo]) {
        let dir_key = path_key(dir);
        for replacement in &self.entries {
            if replacement.parent_key != dir_key {
                continue;
            }
            for entry in entries.iter_mut() {
                if !entry.is_dir && entry.name.eq_ignore_ascii_case(&replacement.file_name) {
                    entry.size = replacement.replacement.data.len() as u64;
                    entry.start_cluster = 0;
                    entry.contiguous = false;
                }
            }
        }
    }
}

struct FileReplacementEntry {
    replacement: VirtualFileReplacement,
    key: String,
    parent_key: String,
    file_name: String,
}

impl FileReplacementEntry {
    fn new(replacement: VirtualFileReplacement) -> Self {
        let key = path_key(&replacement.path);
        let (parent, file_name) = split_normalized_path(&replacement.path);
        Self {
            replacement,
            key,
            parent_key: path_key(&parent),
            file_name,
        }
    }
}

#[repr(C)]
pub struct IsoSimpleFileSystemProtocol {
    protocol: SimpleFileSystemProtocol,
    fs: Rc<VirtualIsoFilesystem>,
    replacements: Rc<FileReplacementSet>,
    volume_size: u64,
    block_size: u32,
}

impl IsoSimpleFileSystemProtocol {
    pub fn install(
        bt: &BootServices,
        handle: Handle,
        fs: Rc<VirtualIsoFilesystem>,
        volume_size: u64,
        block_size: u32,
        replacements: Vec<VirtualFileReplacement>,
    ) -> uefi::Result<RegisteredIsoSimpleFileSystem> {
        let replacement_count = replacements.len();
        let mut protocol = Box::new(Self {
            protocol: SimpleFileSystemProtocol {
                revision: SIMPLE_FILE_SYSTEM_REVISION,
                open_volume: Self::open_volume_handler,
            },
            fs,
            replacements: Rc::new(FileReplacementSet::new(replacements)),
            volume_size,
            block_size,
        });

        let interface = protocol.protocol_ptr().cast::<c_void>();
        unsafe {
            bt.install_protocol_interface(Some(handle), &IsoSimpleFileSystem::GUID, interface)
        }?;

        info!(
            "Installed read-only SimpleFileSystem on virtual ISO handle with {} replacement(s)",
            replacement_count
        );
        Ok(RegisteredIsoSimpleFileSystem { protocol })
    }

    fn protocol_ptr(&mut self) -> *mut SimpleFileSystemProtocol {
        &mut self.protocol
    }

    unsafe extern "efiapi" fn open_volume_handler(
        this: *mut SimpleFileSystemProtocol,
        root: *mut *mut FileProtocol,
    ) -> Status {
        if root.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let Some(protocol) = Self::from_protocol(this) else {
            return Status::INVALID_PARAMETER;
        };

        let mut root_file = IsoFileProtocol::root(
            protocol.fs.clone(),
            protocol.replacements.clone(),
            protocol.volume_size,
            protocol.block_size,
        );
        unsafe {
            *root = root_file.protocol_ptr();
        }
        let _ = Box::into_raw(root_file);
        Status::SUCCESS
    }

    fn from_protocol(this: *mut SimpleFileSystemProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        Some(unsafe { &mut *(this.cast::<Self>()) })
    }
}

pub struct RegisteredIsoSimpleFileSystem {
    protocol: Box<IsoSimpleFileSystemProtocol>,
}

impl RegisteredIsoSimpleFileSystem {
    pub fn leak(self) {
        let _ = Box::leak(self.protocol);
    }
}

fn normalize_path_segments(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path
        .split(|ch| ch == '/' || ch == '\\')
        .filter(|part| !part.is_empty())
    {
        match part {
            "." => {}
            ".." => {
                let _ = parts.pop();
            }
            _ => parts.push(part),
        }
    }

    let mut normalized = String::from("/");
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            normalized.push('/');
        }
        normalized.push_str(part);
    }
    normalized
}

fn path_key(path: &str) -> String {
    let mut key = normalize_path_segments(path);
    key.make_ascii_lowercase();
    key
}

fn split_normalized_path(path: &str) -> (String, String) {
    let normalized = normalize_path_segments(path);
    match normalized.rfind('/') {
        Some(0) => (String::from("/"), normalized[1..].to_string()),
        Some(index) => (
            normalized[..index].to_string(),
            normalized[index + 1..].to_string(),
        ),
        None => (String::from("/"), normalized),
    }
}
