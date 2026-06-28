use core::{ffi::c_void, slice};

use nextboot_fs::FsError;
use uefi::Status;

use super::{build_file_info_bytes, copy_info_response, fs_error_to_status, IsoFileProtocol};

impl IsoFileProtocol {
    pub(super) fn read_regular_file(
        &mut self,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
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

    pub(super) fn read_directory_entry(
        &mut self,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
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
