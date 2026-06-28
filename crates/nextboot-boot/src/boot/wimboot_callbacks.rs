use super::candidates::WIMBOOT_MAX_CALLBACK_PATH;
use super::wimboot_runtime::WimbootRuntimeContext;
use crate::wimboot::{WimbootCallbacks, WimbootVirtualFile};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::ScopedProtocol;

static WIMBOOT_RUNTIME_CONTEXT: AtomicPtr<WimbootRuntimeContext> =
    AtomicPtr::new(core::ptr::null_mut());

pub(super) struct WimbootRuntimeInputs {
    pub(super) runtime_files: Vec<super::wimboot_runtime::WimbootRuntimeFile>,
    pub(super) virtual_files: Vec<WimbootVirtualFile<'static>>,
}

pub(super) struct WimbootRuntimeRegistration<'a> {
    context: *mut WimbootRuntimeContext,
    previous: *mut WimbootRuntimeContext,
    _source_block_io: ScopedProtocol<'a, BlockIO>,
}

impl<'a> WimbootRuntimeRegistration<'a> {
    pub(super) fn install(
        context: WimbootRuntimeContext,
        source_block_io: ScopedProtocol<'a, BlockIO>,
    ) -> Self {
        let context = Box::into_raw(Box::new(context));
        let previous = WIMBOOT_RUNTIME_CONTEXT.swap(context, Ordering::SeqCst);
        Self {
            context,
            previous,
            _source_block_io: source_block_io,
        }
    }

    pub(super) fn callbacks(&self) -> WimbootCallbacks {
        WimbootCallbacks {
            file_size: wimboot_runtime_file_size as usize,
            file_read: wimboot_runtime_file_read as usize,
        }
    }
}

impl Drop for WimbootRuntimeRegistration<'_> {
    fn drop(&mut self) {
        if WIMBOOT_RUNTIME_CONTEXT.load(Ordering::SeqCst) == self.context {
            WIMBOOT_RUNTIME_CONTEXT.store(self.previous, Ordering::SeqCst);
        }

        unsafe {
            drop(Box::from_raw(self.context));
        }
    }
}

extern "C" fn wimboot_runtime_file_size(path: *const u8) -> i32 {
    let Some(context) = current_wimboot_context() else {
        return -1;
    };
    let Some(path) = (unsafe { c_path_bytes(path) }) else {
        return -1;
    };
    let Some(file) = context.find_file(path) else {
        return -1;
    };

    file.size_i32().unwrap_or(-1)
}

extern "C" fn wimboot_runtime_file_read(
    path: *const u8,
    offset: i32,
    len: i32,
    buf: *mut c_void,
) -> i32 {
    if offset < 0 || len < 0 {
        return -1;
    }

    let len = len as usize;
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }

    let Some(context) = current_wimboot_context() else {
        return -1;
    };
    let Some(path) = (unsafe { c_path_bytes(path) }) else {
        return -1;
    };
    let Some(file) = context.find_file(path) else {
        return -1;
    };

    let data = unsafe { core::slice::from_raw_parts_mut(buf.cast::<u8>(), len) };
    match file.read_range(&context.reader, offset as u64, data) {
        Some(()) => len.try_into().unwrap_or(i32::MAX),
        None => -1,
    }
}

fn current_wimboot_context() -> Option<&'static WimbootRuntimeContext> {
    let context = WIMBOOT_RUNTIME_CONTEXT.load(Ordering::SeqCst);
    if context.is_null() {
        None
    } else {
        Some(unsafe { &*context })
    }
}

unsafe fn c_path_bytes(path: *const u8) -> Option<&'static [u8]> {
    if path.is_null() {
        return None;
    }

    for len in 0..WIMBOOT_MAX_CALLBACK_PATH {
        let byte = unsafe { *path.add(len) };
        if byte == 0 {
            return Some(unsafe { core::slice::from_raw_parts(path, len) });
        }
    }

    None
}
