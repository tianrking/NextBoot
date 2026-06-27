//! Read-only Simple File System protocol backed by the selected ISO.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::{ptr, slice};
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::udf::Udf;
use nextboot_fs::{
    FileAttributes as FsFileAttributes, FileInfo as FsFileInfo, FileSystem, FsError,
};
use uefi::proto::unsafe_protocol;
use uefi::table::boot::BootServices;
use uefi::{Guid, Handle, Identify, Status};

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

#[repr(C)]
struct IsoFileProtocol {
    protocol: FileProtocol,
    fs: Rc<VirtualIsoFilesystem>,
    replacements: Rc<FileReplacementSet>,
    path: String,
    info: FsFileInfo,
    replacement_index: Option<usize>,
    position: u64,
    dir_entries: Option<Vec<FsFileInfo>>,
    dir_index: usize,
    volume_size: u64,
    block_size: u32,
}

impl IsoFileProtocol {
    fn root(
        fs: Rc<VirtualIsoFilesystem>,
        replacements: Rc<FileReplacementSet>,
        volume_size: u64,
        block_size: u32,
    ) -> Box<Self> {
        let mut info = FsFileInfo::new(String::new(), 0, true, 0);
        info.attributes = FsFileAttributes::DIRECTORY;
        Self::new(
            fs,
            replacements,
            String::from("/"),
            info,
            None,
            volume_size,
            block_size,
        )
    }

    fn new(
        fs: Rc<VirtualIsoFilesystem>,
        replacements: Rc<FileReplacementSet>,
        path: String,
        info: FsFileInfo,
        replacement_index: Option<usize>,
        volume_size: u64,
        block_size: u32,
    ) -> Box<Self> {
        Box::new(Self {
            protocol: FileProtocol {
                revision: FILE_PROTOCOL_REVISION,
                open: Self::open_handler,
                close: Self::close_handler,
                delete: Self::delete_handler,
                read: Self::read_handler,
                write: Self::write_handler,
                get_position: Self::get_position_handler,
                set_position: Self::set_position_handler,
                get_info: Self::get_info_handler,
                set_info: Self::set_info_handler,
                flush: Self::flush_handler,
                open_ex: Self::open_ex_handler,
                read_ex: Self::read_ex_handler,
                write_ex: Self::write_ex_handler,
                flush_ex: Self::flush_ex_handler,
            },
            fs,
            replacements,
            path,
            info,
            replacement_index,
            position: 0,
            dir_entries: None,
            dir_index: 0,
            volume_size,
            block_size,
        })
    }

    fn protocol_ptr(&mut self) -> *mut FileProtocol {
        &mut self.protocol
    }

    unsafe extern "efiapi" fn open_handler(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        _attributes: u64,
    ) -> Status {
        if new_handle.is_null() || file_name.is_null() {
            return Status::INVALID_PARAMETER;
        }
        if open_mode & EFI_FILE_MODE_READ == 0 {
            return Status::INVALID_PARAMETER;
        }
        if open_mode & (EFI_FILE_MODE_WRITE | EFI_FILE_MODE_CREATE) != 0 {
            return Status::WRITE_PROTECTED;
        }

        let Some(file) = Self::from_protocol(this) else {
            return Status::INVALID_PARAMETER;
        };
        if !file.info.is_dir {
            return Status::NOT_FOUND;
        }

        let Some(requested) = (unsafe { string_from_uefi_char16(file_name) }) else {
            return Status::INVALID_PARAMETER;
        };
        let path = resolve_child_path(&file.path, &requested);
        let mut info = match file.fs.stat(&path) {
            Ok(info) => info,
            Err(err) => return fs_error_to_status(err),
        };
        let replacement_index = if info.is_dir {
            None
        } else {
            file.replacements.find_index(&path)
        };
        if let Some(index) = replacement_index {
            file.replacements.apply_to_file_info(index, &mut info);
            info!(
                "Serving EFI file replacement for {} ({} bytes)",
                path, info.size
            );
        }

        let mut child = Self::new(
            file.fs.clone(),
            file.replacements.clone(),
            path,
            info,
            replacement_index,
            file.volume_size,
            file.block_size,
        );
        unsafe {
            *new_handle = child.protocol_ptr();
        }
        let _ = Box::into_raw(child);
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn open_ex_handler(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
        token: *mut FileIoToken,
    ) -> Status {
        if token.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let status =
            unsafe { Self::open_handler(this, new_handle, file_name, open_mode, attributes) };
        unsafe {
            (*token).status = status;
        }
        status
    }

    unsafe extern "efiapi" fn close_handler(this: *mut FileProtocol) -> Status {
        if this.is_null() {
            return Status::INVALID_PARAMETER;
        }

        unsafe {
            drop(Box::from_raw(this.cast::<Self>()));
        }
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn delete_handler(this: *mut FileProtocol) -> Status {
        let status = unsafe { Self::close_handler(this) };
        if status == Status::SUCCESS {
            Status::WARN_DELETE_FAILURE
        } else {
            status
        }
    }

    unsafe extern "efiapi" fn read_handler(
        this: *mut FileProtocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        if buffer_size.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let Some(file) = Self::from_protocol(this) else {
            return Status::INVALID_PARAMETER;
        };

        if file.info.is_dir {
            file.read_directory_entry(buffer_size, buffer)
        } else {
            file.read_regular_file(buffer_size, buffer)
        }
    }

    unsafe extern "efiapi" fn read_ex_handler(
        this: *mut FileProtocol,
        token: *mut FileIoToken,
    ) -> Status {
        if token.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let status =
            unsafe { Self::read_handler(this, &mut (*token).buffer_size, (*token).buffer) };
        unsafe {
            (*token).status = status;
        }
        status
    }

    unsafe extern "efiapi" fn write_handler(
        _this: *mut FileProtocol,
        buffer_size: *mut usize,
        _buffer: *const c_void,
    ) -> Status {
        if !buffer_size.is_null() {
            unsafe {
                *buffer_size = 0;
            }
        }
        Status::WRITE_PROTECTED
    }

    unsafe extern "efiapi" fn write_ex_handler(
        this: *mut FileProtocol,
        token: *mut FileIoToken,
    ) -> Status {
        if token.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let status = unsafe {
            Self::write_handler(
                this,
                &mut (*token).buffer_size,
                (*token).buffer.cast_const(),
            )
        };
        unsafe {
            (*token).status = status;
        }
        status
    }

    unsafe extern "efiapi" fn get_position_handler(
        this: *const FileProtocol,
        position: *mut u64,
    ) -> Status {
        if position.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let Some(file) = Self::from_protocol(this.cast_mut()) else {
            return Status::INVALID_PARAMETER;
        };
        unsafe {
            *position = file.position;
        }
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn set_position_handler(
        this: *mut FileProtocol,
        position: u64,
    ) -> Status {
        let Some(file) = Self::from_protocol(this) else {
            return Status::INVALID_PARAMETER;
        };

        if file.info.is_dir {
            if position != 0 {
                return Status::UNSUPPORTED;
            }
            file.position = 0;
            file.dir_index = 0;
            return Status::SUCCESS;
        }

        let target = if position == u64::MAX {
            file.info.size
        } else {
            position
        };
        if target > file.info.size {
            return Status::INVALID_PARAMETER;
        }

        file.position = target;
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn get_info_handler(
        this: *mut FileProtocol,
        information_type: *const Guid,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        if information_type.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let Some(file) = Self::from_protocol(this) else {
            return Status::INVALID_PARAMETER;
        };
        let guid = unsafe { *information_type };

        let bytes = if guid == EFI_FILE_INFO_GUID {
            build_file_info_bytes(&file.info, file.block_size)
        } else if guid == EFI_FILE_SYSTEM_INFO_GUID {
            build_file_system_info_bytes(file.volume_size, file.block_size)
        } else if guid == EFI_FILE_SYSTEM_VOLUME_LABEL_GUID {
            build_volume_label_bytes()
        } else {
            return Status::UNSUPPORTED;
        };

        match bytes {
            Ok(bytes) => unsafe { copy_info_response(&bytes, buffer_size, buffer) },
            Err(status) => status,
        }
    }

    unsafe extern "efiapi" fn set_info_handler(
        _this: *mut FileProtocol,
        _information_type: *const Guid,
        _buffer_size: usize,
        _buffer: *const c_void,
    ) -> Status {
        Status::WRITE_PROTECTED
    }

    unsafe extern "efiapi" fn flush_handler(_this: *mut FileProtocol) -> Status {
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn flush_ex_handler(
        this: *mut FileProtocol,
        token: *mut FileIoToken,
    ) -> Status {
        if token.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let status = unsafe { Self::flush_handler(this) };
        unsafe {
            (*token).status = status;
        }
        status
    }

    fn from_protocol(this: *mut FileProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        Some(unsafe { &mut *(this.cast::<Self>()) })
    }

    fn read_regular_file(&mut self, buffer_size: *mut usize, buffer: *mut c_void) -> Status {
        let requested = unsafe { *buffer_size };
        if requested == 0 {
            unsafe {
                *buffer_size = 0;
            }
            return Status::SUCCESS;
        }
        if buffer.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let dst = unsafe { slice::from_raw_parts_mut(buffer.cast::<u8>(), requested) };
        let read_result = if let Some(index) = self.replacement_index {
            self.read_replacement_file(index, dst)
        } else {
            self.fs.read_file(&self.path, self.position, dst)
        };

        match read_result {
            Ok(read) => {
                self.position = self.position.saturating_add(read as u64);
                unsafe {
                    *buffer_size = read;
                }
                Status::SUCCESS
            }
            Err(err) => fs_error_to_status(err),
        }
    }

    fn read_directory_entry(&mut self, buffer_size: *mut usize, buffer: *mut c_void) -> Status {
        if self.dir_entries.is_none() {
            match self.fs.read_dir(&self.path) {
                Ok(mut entries) => {
                    if !self.replacements.is_empty() {
                        self.replacements
                            .apply_to_directory(&self.path, &mut entries);
                    }
                    self.dir_entries = Some(entries);
                }
                Err(err) => return fs_error_to_status(err),
            }
        }

        let Some(entries) = self.dir_entries.as_ref() else {
            return Status::DEVICE_ERROR;
        };
        if self.dir_index >= entries.len() {
            unsafe {
                *buffer_size = 0;
            }
            return Status::SUCCESS;
        }

        let entry = &entries[self.dir_index];
        let bytes = match build_file_info_bytes(entry, self.block_size) {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        let status = unsafe { copy_info_response(&bytes, buffer_size, buffer) };
        if status == Status::SUCCESS {
            self.dir_index += 1;
            self.position = self.dir_index as u64;
        }

        status
    }

    fn read_replacement_file(&self, index: usize, dst: &mut [u8]) -> Result<usize, FsError> {
        let Some(data) = self.replacements.data(index) else {
            return Err(FsError::FileNotFound);
        };
        if self.position >= data.len() as u64 {
            return Ok(0);
        }

        let start = usize::try_from(self.position).map_err(|_| FsError::FileTooLarge)?;
        let available = data.len().saturating_sub(start);
        let to_copy = available.min(dst.len());
        dst[..to_copy].copy_from_slice(&data[start..start + to_copy]);
        Ok(to_copy)
    }
}

fn build_file_info_bytes(info: &FsFileInfo, block_size: u32) -> Result<Vec<u8>, Status> {
    let mut name = encode_utf16_nul(&info.name)?;
    if name.is_empty() {
        name.push(0);
    }

    let name_bytes = name.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let unaligned_size = EFI_FILE_INFO_NAME_OFFSET
        .checked_add(name_bytes)
        .ok_or(Status::OUT_OF_RESOURCES)?;
    let total_size = align_up(unaligned_size, 8).ok_or(Status::OUT_OF_RESOURCES)?;
    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;

    push_u64(&mut data, total_size as u64);
    push_u64(&mut data, info.size);
    push_u64(&mut data, physical_size(info.size, block_size));
    append_efi_time(&mut data);
    append_efi_time(&mut data);
    append_efi_time(&mut data);
    push_u64(&mut data, efi_file_attributes(info));
    debug_assert_eq!(data.len(), EFI_FILE_INFO_NAME_OFFSET);
    append_utf16(&mut data, &name);
    data.resize(total_size, 0);
    Ok(data)
}

fn build_file_system_info_bytes(volume_size: u64, block_size: u32) -> Result<Vec<u8>, Status> {
    let label = encode_utf16_nul(VOLUME_LABEL)?;
    let label_bytes = label.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let unaligned_size = EFI_FILE_SYSTEM_INFO_LABEL_OFFSET
        .checked_add(label_bytes)
        .ok_or(Status::OUT_OF_RESOURCES)?;
    let total_size = align_up(unaligned_size, 8).ok_or(Status::OUT_OF_RESOURCES)?;

    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    push_u64(&mut data, total_size as u64);
    data.push(1);
    data.extend_from_slice(&[0; 7]);
    push_u64(&mut data, volume_size);
    push_u64(&mut data, 0);
    push_u32(&mut data, block_size);
    debug_assert_eq!(data.len(), EFI_FILE_SYSTEM_INFO_LABEL_OFFSET);
    append_utf16(&mut data, &label);
    data.resize(total_size, 0);
    Ok(data)
}

fn build_volume_label_bytes() -> Result<Vec<u8>, Status> {
    let label = encode_utf16_nul(VOLUME_LABEL)?;
    let total_size = label.len().checked_mul(2).ok_or(Status::OUT_OF_RESOURCES)?;
    let mut data = Vec::new();
    data.try_reserve_exact(total_size)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    append_utf16(&mut data, &label);
    Ok(data)
}

unsafe fn copy_info_response(bytes: &[u8], buffer_size: *mut usize, buffer: *mut c_void) -> Status {
    if buffer_size.is_null() {
        return Status::INVALID_PARAMETER;
    }

    let provided = unsafe { *buffer_size };
    unsafe {
        *buffer_size = bytes.len();
    }
    if buffer.is_null() || provided < bytes.len() {
        return Status::BUFFER_TOO_SMALL;
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
    }
    Status::SUCCESS
}

fn encode_utf16_nul(value: &str) -> Result<Vec<u16>, Status> {
    let mut out = Vec::new();
    out.try_reserve_exact(value.len().saturating_add(1))
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    for unit in value.encode_utf16() {
        out.push(unit);
    }
    out.push(0);
    Ok(out)
}

fn append_utf16(data: &mut Vec<u8>, units: &[u16]) {
    for unit in units {
        data.extend_from_slice(&unit.to_le_bytes());
    }
}

fn append_efi_time(data: &mut Vec<u8>) {
    data.extend_from_slice(&[0; 16]);
}

fn physical_size(file_size: u64, block_size: u32) -> u64 {
    let block_size = u64::from(block_size);
    if file_size == 0 || block_size == 0 {
        return file_size;
    }

    file_size
        .checked_add(block_size - 1)
        .map(|size| (size / block_size) * block_size)
        .unwrap_or(file_size)
}

fn efi_file_attributes(info: &FsFileInfo) -> u64 {
    let mut attrs = EFI_FILE_ATTR_READ_ONLY;
    if info.is_dir {
        attrs |= EFI_FILE_ATTR_DIRECTORY;
    } else {
        attrs |= EFI_FILE_ATTR_ARCHIVE;
    }
    if info.attributes.contains(FsFileAttributes::HIDDEN) {
        attrs |= EFI_FILE_ATTR_HIDDEN;
    }
    if info.attributes.contains(FsFileAttributes::SYSTEM) {
        attrs |= EFI_FILE_ATTR_SYSTEM;
    }
    attrs
}

unsafe fn string_from_uefi_char16(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }

    let mut out = String::new();
    for index in 0..4096 {
        let unit = unsafe { ptr::read_unaligned(ptr.add(index)) };
        if unit == 0 {
            return Some(out);
        }

        let ch = char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}');
        out.push(if ch == '\\' { '/' } else { ch });
    }

    warn!("UEFI file path was not NUL terminated within 4096 UTF-16 code units");
    None
}

fn resolve_child_path(base: &str, requested: &str) -> String {
    if requested.is_empty() || requested == "." {
        return normalize_path_segments(base);
    }

    if requested.starts_with('/') || requested.starts_with('\\') {
        return normalize_path_segments(requested);
    }

    let mut combined = String::new();
    combined.push_str(base.trim_end_matches(['/', '\\']));
    if combined.is_empty() {
        combined.push('/');
    }
    if !combined.ends_with('/') {
        combined.push('/');
    }
    combined.push_str(requested);
    normalize_path_segments(&combined)
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

fn fs_error_to_status(err: FsError) -> Status {
    match err {
        FsError::FileNotFound | FsError::DirectoryNotFound => Status::NOT_FOUND,
        FsError::InvalidPath | FsError::InvalidArgument => Status::INVALID_PARAMETER,
        FsError::OutOfMemory | FsError::FileTooLarge => Status::OUT_OF_RESOURCES,
        FsError::NotDirectory | FsError::NotFile | FsError::UnsupportedFs => Status::UNSUPPORTED,
        FsError::InvalidSignature | FsError::BlockSizeMismatch | FsError::Corrupted => {
            Status::VOLUME_CORRUPTED
        }
        FsError::ReadError => Status::DEVICE_ERROR,
    }
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}
