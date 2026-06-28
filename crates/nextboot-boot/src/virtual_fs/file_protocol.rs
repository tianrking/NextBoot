use super::info::{
    build_file_info_bytes, build_file_system_info_bytes, build_volume_label_bytes,
    copy_info_response, fs_error_to_status, resolve_child_path, string_from_uefi_char16,
};
use super::*;

mod read;

#[repr(C)]
pub(super) struct IsoFileProtocol {
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
    pub(super) fn root(
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

    pub(super) fn protocol_ptr(&mut self) -> *mut FileProtocol {
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
}
