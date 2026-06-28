use super::util::normalize_iso_path;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use log::info;
use nextboot_virtio::protocol::{DevicePathHeader, DevicePathType, EndDevicePath, MediaSubtype};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::unsafe_protocol;
use uefi::table::boot::BootServices;
use uefi::{Handle, Identify, Status};

const LINUX_EFI_INITRD_MEDIA_GUID: [u8; 16] = [
    0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, 0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68,
];

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("56ec3091-954c-11d2-8e3f-00a0c969723b")]
struct LoadFile(LoadFileProtocol);

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("4006c0c1-fcb3-403e-996d-4a6c8724e06d")]
struct LoadFile2(LoadFile2Protocol);

#[derive(Debug)]
#[repr(C)]
struct LoadFileProtocol {
    load_file: extern "efiapi" fn(
        *mut LoadFileProtocol,
        *const FfiDevicePath,
        bool,
        *mut usize,
        *mut c_void,
    ) -> Status,
}

#[derive(Debug)]
#[repr(C)]
struct LoadFile2Protocol {
    load_file: extern "efiapi" fn(
        *mut LoadFile2Protocol,
        *const FfiDevicePath,
        bool,
        *mut usize,
        *mut c_void,
    ) -> Status,
}

#[derive(Debug)]
#[repr(transparent)]
#[unsafe_protocol("5b1b31a1-9562-11d2-8e3f-00a0c969723b")]
pub(super) struct RawLoadedImage(pub(super) RawLoadedImageProtocol);

#[derive(Debug)]
#[repr(C)]
pub(super) struct RawLoadedImageProtocol {
    pub(super) revision: u32,
    pub(super) parent_handle: *mut c_void,
    pub(super) system_table: *const c_void,
    pub(super) device_handle: *mut c_void,
    pub(super) file_path: *const FfiDevicePath,
    pub(super) reserved: *const c_void,
    pub(super) load_options_size: u32,
    pub(super) load_options: *const c_void,
    pub(super) image_base: *const c_void,
    pub(super) image_size: u64,
    pub(super) image_code_type: uefi::table::boot::MemoryType,
    pub(super) image_data_type: uefi::table::boot::MemoryType,
    pub(super) unload: Option<unsafe extern "efiapi" fn(*mut c_void) -> Status>,
}

pub(super) struct PreloadedFile {
    pub(super) path: String,
    pub(super) data: Vec<u8>,
}

#[repr(C)]
pub(super) struct PreloadedLoadFileProtocol {
    load_file: LoadFileProtocol,
    load_file_2: LoadFile2Protocol,
    entries: Vec<PreloadedFile>,
}

impl PreloadedLoadFileProtocol {
    pub(super) fn install(
        bt: &BootServices,
        handle: Handle,
        entries: Vec<PreloadedFile>,
    ) -> uefi::Result<RegisteredPreloadedLoadFile> {
        let mut protocol = Box::new(Self {
            load_file: LoadFileProtocol {
                load_file: Self::load_file_handler,
            },
            load_file_2: LoadFile2Protocol {
                load_file: Self::load_file_2_handler,
            },
            entries,
        });

        let load_file_interface = protocol.load_file_ptr().cast::<c_void>();
        unsafe {
            bt.install_protocol_interface(Some(handle), &LoadFile::GUID, load_file_interface)
        }?;

        let load_file_2_interface = protocol.load_file_2_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &LoadFile2::GUID, load_file_2_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &LoadFile::GUID, load_file_interface)
            };
            return Err(err);
        }

        Ok(RegisteredPreloadedLoadFile { protocol })
    }

    fn load_file_ptr(&mut self) -> *mut LoadFileProtocol {
        &mut self.load_file
    }

    fn load_file_2_ptr(&mut self) -> *mut LoadFile2Protocol {
        &mut self.load_file_2
    }

    extern "efiapi" fn load_file_handler(
        this: *mut LoadFileProtocol,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        let Some(protocol) = Self::from_load_file(this) else {
            return Status::INVALID_PARAMETER;
        };

        protocol.load_file(file_path, boot_policy, buffer_size, buffer)
    }

    extern "efiapi" fn load_file_2_handler(
        this: *mut LoadFile2Protocol,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        let Some(protocol) = Self::from_load_file_2(this) else {
            return Status::INVALID_PARAMETER;
        };

        protocol.load_file(file_path, boot_policy, buffer_size, buffer)
    }

    fn load_file(
        &self,
        file_path: *const FfiDevicePath,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        if buffer_size.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let requested = unsafe { load_file_path_from_device_path(file_path) };
        let entry = requested
            .as_ref()
            .and_then(|path| self.find_entry(path))
            .or_else(|| boot_policy.then(|| self.entries.first()).flatten());

        let Some(entry) = entry else {
            return Status::NOT_FOUND;
        };

        let required_size = entry.data.len();
        let provided_size = unsafe { *buffer_size };
        unsafe {
            *buffer_size = required_size;
        }

        if buffer.is_null() || provided_size < required_size {
            return Status::BUFFER_TOO_SMALL;
        }

        unsafe {
            ptr::copy_nonoverlapping(entry.data.as_ptr(), buffer.cast::<u8>(), required_size);
        }

        Status::SUCCESS
    }

    fn find_entry(&self, path: &str) -> Option<&PreloadedFile> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    fn from_load_file(this: *mut LoadFileProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        Some(unsafe { &mut *(this.cast::<Self>()) })
    }

    fn from_load_file_2(this: *mut LoadFile2Protocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, load_file_2);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
    }
}

pub(super) struct RegisteredPreloadedLoadFile {
    protocol: Box<PreloadedLoadFileProtocol>,
}

impl RegisteredPreloadedLoadFile {
    pub(super) fn leak(self) {
        let _ = Box::leak(self.protocol);
    }
}

#[repr(C, packed)]
struct VendorMediaDevicePath {
    header: DevicePathHeader,
    guid: [u8; 16],
}

impl VendorMediaDevicePath {
    fn new(guid: [u8; 16]) -> Self {
        Self {
            header: DevicePathHeader::new(DevicePathType::MEDIA, MediaSubtype::Vendor as u8, 20),
            guid,
        }
    }
}

#[repr(C)]
pub(super) struct LinuxInitrdLoadFile2Protocol {
    load_file_2: LoadFile2Protocol,
    data: Vec<u8>,
}

impl LinuxInitrdLoadFile2Protocol {
    pub(super) fn install(
        bt: &BootServices,
        data: Vec<u8>,
    ) -> uefi::Result<RegisteredLinuxInitrdLoadFile2> {
        let mut protocol = Box::new(Self {
            load_file_2: LoadFile2Protocol {
                load_file: Self::load_file_2_handler,
            },
            data,
        });
        let load_file_2_interface = protocol.load_file_2_ptr().cast::<c_void>();
        let handle = unsafe {
            bt.install_protocol_interface(None, &LoadFile2::GUID, load_file_2_interface)
        }?;

        let mut device_path = linux_initrd_media_device_path_bytes().into_boxed_slice();
        let device_path_interface = device_path.as_mut_ptr().cast::<c_void>();
        if let Err(err) = unsafe {
            bt.install_protocol_interface(Some(handle), &DevicePath::GUID, device_path_interface)
        } {
            let _ = unsafe {
                bt.uninstall_protocol_interface(handle, &LoadFile2::GUID, load_file_2_interface)
            };
            return Err(err);
        }

        Ok(RegisteredLinuxInitrdLoadFile2 {
            protocol,
            device_path,
        })
    }

    fn load_file_2_ptr(&mut self) -> *mut LoadFile2Protocol {
        &mut self.load_file_2
    }

    extern "efiapi" fn load_file_2_handler(
        this: *mut LoadFile2Protocol,
        _file_path: *const FfiDevicePath,
        _boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        let Some(protocol) = Self::from_load_file_2(this) else {
            return Status::INVALID_PARAMETER;
        };

        protocol.load_file(buffer_size, buffer)
    }

    fn load_file(&self, buffer_size: *mut usize, buffer: *mut c_void) -> Status {
        if buffer_size.is_null() {
            return Status::INVALID_PARAMETER;
        }

        let required_size = self.data.len();
        let provided_size = unsafe { *buffer_size };
        unsafe {
            *buffer_size = required_size;
        }
        info!(
            "Linux initrd LoadFile2 request: required={} provided={} buffer_null={}",
            required_size,
            provided_size,
            buffer.is_null()
        );

        if buffer.is_null() || provided_size < required_size {
            return Status::BUFFER_TOO_SMALL;
        }

        unsafe {
            ptr::copy_nonoverlapping(self.data.as_ptr(), buffer.cast::<u8>(), required_size);
        }
        info!("Linux initrd LoadFile2 served {} bytes", required_size);

        Status::SUCCESS
    }

    fn from_load_file_2(this: *mut LoadFile2Protocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        Some(unsafe { &mut *(this.cast::<Self>()) })
    }
}

pub(super) struct RegisteredLinuxInitrdLoadFile2 {
    protocol: Box<LinuxInitrdLoadFile2Protocol>,
    device_path: Box<[u8]>,
}

impl RegisteredLinuxInitrdLoadFile2 {
    pub(super) fn leak(self) {
        let _ = Box::leak(self.protocol);
        let _ = Box::leak(self.device_path);
    }
}

pub(super) fn normalize_load_file_key(path: &str) -> String {
    let mut normalized = normalize_iso_path(path);
    normalized.make_ascii_lowercase();
    normalized
}

fn linux_initrd_media_device_path_bytes() -> Vec<u8> {
    let vendor = VendorMediaDevicePath::new(LINUX_EFI_INITRD_MEDIA_GUID);
    let end = EndDevicePath::new();
    let mut data = Vec::new();

    unsafe {
        let vendor_bytes = core::slice::from_raw_parts(
            &vendor as *const VendorMediaDevicePath as *const u8,
            core::mem::size_of::<VendorMediaDevicePath>(),
        );
        data.extend_from_slice(vendor_bytes);
    }

    data.extend_from_slice(&end.to_bytes());
    data
}

unsafe fn load_file_path_from_device_path(file_path: *const FfiDevicePath) -> Option<String> {
    if file_path.is_null() {
        return None;
    }

    let mut node = file_path.cast::<u8>();
    let mut path = String::new();

    for _ in 0..64 {
        let node_type = unsafe { *node };
        let node_subtype = unsafe { *node.add(1) };
        let length = u16::from_le_bytes([unsafe { *node.add(2) }, unsafe { *node.add(3) }]);
        let length = usize::from(length);

        if length < 4 {
            return None;
        }

        if node_type == 0x7f {
            break;
        }

        if node_type == 0x04 && node_subtype == 0x04 {
            let units = (length - 4) / 2;
            let chars = unsafe { node.add(4).cast::<u16>() };

            for index in 0..units {
                let unit = unsafe { ptr::read_unaligned(chars.add(index)) };
                if unit == 0 {
                    break;
                }

                let ch = char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}');
                path.push(if ch == '\\' { '/' } else { ch });
            }
        }

        node = unsafe { node.add(length) };
    }

    if path.is_empty() {
        None
    } else {
        Some(normalize_load_file_key(&path))
    }
}
