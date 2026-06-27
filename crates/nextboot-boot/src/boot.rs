//! 引导管理模块
//!
//! 负责准备和执行 ISO 引导

use crate::scanner::{ImageFormat, IsoExtent, IsoFile, OsType};
use crate::vdi;
use crate::ventoy_linux::{VentoyDudFile, VentoyLinuxInitrdInput};
use crate::vhdx;
use crate::virtual_fs::{
    IsoSimpleFileSystemProtocol, RegisteredIsoSimpleFileSystem, VirtualFileReplacement,
    VirtualIsoFilesystem,
};
use crate::wim;
use crate::wimboot::{self, WimbootCallbacks, WimbootVirtualFile};
use crate::xz::{self, XzDecodeError};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::udf::Udf;
use nextboot_fs::{detect_fs_type, BlockIoOps, FileExtent, FileSystemType, FsError, SharedBlockIo};
use nextboot_virtio::mapping::ByteMappingTable;
use nextboot_virtio::protocol::append_file_path_device_path;
use nextboot_virtio::{
    MemoryOverlay, PhysicalReader, VirtIoError, VirtualBlockIo, VirtualDeviceConfig,
    VirtualDeviceType,
};
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{
    BootServices, LoadImageSource, MemoryType, OpenProtocolAttributes, OpenProtocolParams,
    ScopedProtocol, SearchType,
};
use uefi::table::runtime::{RuntimeServices, VariableAttributes, VariableVendor};
use uefi::{CString16, Guid, Handle, Identify, Status};

mod load_file;
mod source_volume;
mod vhd;
use load_file::{
    normalize_load_file_key, LinuxInitrdLoadFile2Protocol, PreloadedFile,
    PreloadedLoadFileProtocol, RawLoadedImage,
};
use source_volume::{
    IsoMappedFileMetadata, SourceVolumeFile, SourceVolumeFileMetadata, SourceVolumeFileSystem,
    SourceVolumeReader, ZeroPhysicalReader,
};

const EFI_BOOT_X64: &str = "\\EFI\\BOOT\\BOOTX64.EFI";
const EFI_BOOT_AA64: &str = "\\EFI\\BOOT\\BOOTAA64.EFI";
const EFI_BOOT_IA32: &str = "\\EFI\\BOOT\\BOOTIA32.EFI";
const EFI_BOOT_ARM: &str = "\\EFI\\BOOT\\BOOTARM.EFI";
const WINDOWS_BOOTMGFW_PATH: &str = "/efi/microsoft/boot/bootmgfw.efi";
const NEXTBOOT_OS_PARAM_NAME: &str = "NextBootOsParam";
const NEXTBOOT_OS_PARAM_VENDOR_GUID: Guid = uefi::guid!("c1775af2-4211-4f55-9f6f-2cc5ef5667f0");
const VENTOY_OS_PARAM_VENDOR_GUID: Guid = uefi::guid!("77772020-2e77-6576-6e74-6f792e6e6574");
const NEXTBOOT_OS_PARAM_MAGIC: &[u8; 8] = b"NBOSPARM";
const NEXTBOOT_OS_PARAM_VERSION: u16 = 1;
const NEXTBOOT_OS_PARAM_HEADER_SIZE: usize = 80;
const NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE: usize = 24;
const NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT: u16 = 0x0001;
const NEXTBOOT_OS_PARAM_FLAG_EL_TORITO: u16 = 0x0002;
const VENTOY_RUNTIME_ALIGNMENT: usize = 4096;
const WIMBOOT_MAX_CALLBACK_PATH: usize = 512;
const WIMBOOT_BOOT_WIM_CALLBACK_PATH: &str = "nb-boot-wim";
const WIMBOOT_BCD_CALLBACK_PATH: &str = "nb-bcd";
const WIMBOOT_BOOT_SDI_CALLBACK_PATH: &str = "nb-boot-sdi";
const WIMBOOT_BOOTMGFW_CALLBACK_PATH: &str = "nb-bootmgfw";
const WIMBOOT_SELF_CALLBACK_PATH: &str = "nb-wimboot";
const WIMBOOT_WINPESHL_CALLBACK_PATH: &str = "nb-winpeshl";
const WIMBOOT_XZ_MAX_OUTPUT_SIZE: usize = 2 * 1024 * 1024;
const VTOYJUMP_CANDIDATES: &[&str] = &["/ventoy/vtoyjump64.exe"];
const VENTOY_COMMON_CPIO_CANDIDATES: &[&str] = &["/ventoy/ventoy.cpio"];
const LINUX_CONFIG_MAX_SIZE: usize = 512 * 1024;
const VENTOY_CONF_REPLACE_MAX_SIZE: usize = 1024 * 1024;
const ISO9660_SECTOR_SIZE: u64 = 2048;
const LINUX_GRUB_CONFIG_CANDIDATES: &[&str] = &[
    "/boot/grub/grub.cfg",
    "/boot/grub/loopback.cfg",
    "/grub/grub.cfg",
    "/EFI/BOOT/grub.cfg",
    "/efi/boot/grub.cfg",
    "/boot/grub/kernels.cfg",
];
const LINUX_ISOLINUX_CONFIG_CANDIDATES: &[&str] = &[
    "/isolinux/isolinux.cfg",
    "/isolinux/syslinux.cfg",
    "/syslinux/syslinux.cfg",
    "/boot/isolinux/isolinux.cfg",
    "/boot/syslinux/syslinux.cfg",
    "/boot/isolinux/syslinux.cfg",
];
const LINUX_LOADER_ENTRY_DIRS: &[&str] = &["/loader/entries", "/boot/loader/entries"];
const LINUX_KERNEL_CANDIDATES: &[&str] = &[
    "/casper/vmlinuz",
    "/casper/vmlinuz.efi",
    "/live/vmlinuz",
    "/boot/vmlinuz",
    "/boot/vmlinuz-x86_64",
    "/arch/boot/x86_64/vmlinuz-linux",
    "/blackarch/boot/x86_64/vmlinuz-linux",
    "/images/pxeboot/vmlinuz",
    "/boot/x86_64/loader/linux",
    "/isolinux/vmlinuz",
    "/boot/isolinux/vmlinuz",
    "/syslinux/vmlinuz",
    "/EFI/BOOT/vmlinuz",
];
const LINUX_INITRD_CANDIDATES: &[&str] = &[
    "/boot/all.rdz",
    "/casper/initrd",
    "/casper/initrd.gz",
    "/casper/initrd-oem",
    "/boot/grub/initrd.xz",
    "/initrd.gz",
    "/slax/boot/initrfs.img",
    "/minios/boot/initrfs.img",
    "/pmagic/initrd.img",
    "/boot/initrd.xz",
    "/boot/initrd.gz",
    "/boot/initrd",
    "/boot/x86_64/loader/initrd",
    "/boot/initramfs-x86_64.img",
    "/boot/isolinux/initramfs_data64.cpio.gz",
    "/boot/initrd.img",
    "/isolinux/initrd.gz",
    "/images/pxeboot/initrd.img",
    "/Setup/initrd.gz",
    "/isolinux/initramfs",
    "/boot/iniramfs.igz",
    "/initrd-x86_64",
    "/live/initrd.img",
    "/initrd.img",
    "/sysresccd/boot/x86_64/sysresccd.img",
    "/CDlinux/initrd",
    "/parabola/boot/x86_64/parabolaiso.img",
    "/parabola/boot/x86_64/initramfs-linux-libre.img",
    "/hyperbola/boot/x86_64/hyperiso.img",
    "/EFI/BOOT/initrd.img",
    "/initrd",
    "/live/initrd1",
    "/isolinux/initrd.img",
    "/syslinux/kernel/initramfs.gz",
    "/boot/rootfs.xz",
    "/arch/boot/x86_64/archiso.img",
    "/blackarch/boot/x86_64/archiso.img",
    "/blackarch/boot/x86_64/initramfs-linux.img",
    "/live/initrd2.img",
    "/install.amd/initrd.gz",
    "/install.amd/gtk/initrd.gz",
    "/austrumi/initrd.gz",
    "/boot/initfs.x86_64-efi",
    "/boot/initfs.i386-pc",
    "/antiX/initrd.gz",
    "/360Disk/initrd.gz",
    "/porteus/initrd.xz",
    "/pyabr/boot/initrfs.img",
    "/initrd0.img",
    "/sysresccd/boot/i686/sysresccd.img",
    "/boot/full.cz",
    "/live/initrd",
    "/initramfs-linux.img",
    "/boot/isolinux/initrd.gz",
];
const WIMBOOT_BCD_CANDIDATES: &[&str] = &[
    "/ventoy/common_bcd",
    "/ventoy/bcd",
    "/boot/bcd",
    "/efi/microsoft/boot/bcd",
];
const WIMBOOT_COMPRESSED_BCD_CANDIDATES: &[&str] = &["/ventoy/common_bcd.xz", "/ventoy/bcd.xz"];
const WIMBOOT_BOOT_SDI_CANDIDATES: &[&str] = &[
    "/boot/boot.sdi",
    "/2K10/FONTS/boot.sdi",
    "/SSTR/boot.sdi",
    "/ISPE/BOOT.SDI",
    "/boot/uqi.sdi",
    "/ISYL/boot.sdi",
    "/WEPE/WEPE.SDI",
];
const WINDOWS_ISO_BOOT_WIM_CANDIDATES: &[&str] = &[
    "/sources/boot.wim",
    "/boot/boot.wim",
    "/x64/sources/boot.wim",
    "/x86/sources/boot.wim",
];
const WINDOWS_ISO_BCD_CANDIDATES: &[&str] = &[
    "/boot/bcd",
    "/efi/microsoft/boot/bcd",
    "/x64/boot/bcd",
    "/x86/boot/bcd",
];
const WINDOWS_ISO_BOOT_SDI_CANDIDATES: &[&str] = &[
    "/boot/boot.sdi",
    "/x64/boot/boot.sdi",
    "/x86/boot/boot.sdi",
    "/2K10/FONTS/boot.sdi",
    "/SSTR/boot.sdi",
    "/ISPE/BOOT.SDI",
    "/boot/uqi.sdi",
    "/ISYL/boot.sdi",
    "/WEPE/WEPE.SDI",
];
const WIMBOOT_BOOTMGFW_VIRTUAL_NAME: &str = "bootmgfw.efi";
const WIMBOOT_WINPESHL_VIRTUAL_NAME: &str = "winpeshl.exe";
const WIMBOOT_WIM_BOOTMGFW_CANDIDATES: &[&str] = &["\\Windows\\Boot\\EFI\\bootmgfw.efi"];
const WIMBOOT_WIM_BCD_CANDIDATES: &[&str] = &["\\Windows\\Boot\\DVD\\EFI\\BCD"];
const WIMBOOT_WIM_BOOT_SDI_CANDIDATES: &[&str] = &[
    "\\Windows\\Boot\\DVD\\EFI\\boot.sdi",
    "\\sms\\boot\\boot.sdi",
];
const WIMBOOT_WIM_WINPESHL_CANDIDATES: &[&str] = &["\\Windows\\System32\\winpeshl.exe"];

static WIMBOOT_RUNTIME_CONTEXT: AtomicPtr<WimbootRuntimeContext> = AtomicPtr::new(ptr::null_mut());

fn default_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }

    #[cfg(target_arch = "arm")]
    {
        &[EFI_BOOT_ARM, EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32]
    }

    #[cfg(target_arch = "x86")]
    {
        &[EFI_BOOT_IA32, EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_ARM]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }
}

fn generic_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(target_arch = "arm")]
    {
        &[
            "/efi/boot/bootarm.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
        ]
    }

    #[cfg(target_arch = "x86")]
    {
        &[
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }
}

fn wimboot_helper_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "x86_64")]
    {
        &[
            "/ventoy/wimboot.x86_64",
            "/ventoy/wimboot.x86_64.efi",
            "/ventoy/wimboot_x64.efi",
            "/ventoy/wimboot.efi",
        ]
    }

    #[cfg(target_arch = "x86")]
    {
        &[
            "/ventoy/wimboot.i386.efi",
            "/ventoy/wimboot.i386",
            "/ventoy/wimboot_ia32.efi",
            "/ventoy/wimboot.efi",
        ]
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        &[]
    }
}

fn compressed_wimboot_helper_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "x86_64")]
    {
        &["/ventoy/wimboot.x86_64.xz"]
    }

    #[cfg(target_arch = "x86")]
    {
        &["/ventoy/wimboot.i386.efi.xz"]
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        &[]
    }
}

fn ventoy_arch_cpio_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &["/ventoy/ventoy_arm64.cpio"]
    }

    #[cfg(target_arch = "x86")]
    {
        &["/ventoy/ventoy_x86.cpio"]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86")))]
    {
        &["/ventoy/ventoy_x86.cpio"]
    }
}

/// 引导管理器
pub struct BootManager<'a> {
    bt: &'a BootServices,
    rt: &'a RuntimeServices,
    parent_image: Handle,
    iso: &'a IsoFile,
}

impl<'a> BootManager<'a> {
    /// 创建新的引导管理器
    pub fn new(
        bt: &'a BootServices,
        rt: &'a RuntimeServices,
        parent_image: Handle,
        iso: &'a IsoFile,
    ) -> Self {
        Self {
            bt,
            rt,
            parent_image,
            iso,
        }
    }

    /// 准备并执行引导
    pub fn prepare_and_boot(&self) -> uefi::Result<()> {
        info!("Preparing to boot: {}", self.iso.path);
        if self.iso.image_format.is_efi_executable() {
            return self.boot_efi_executable();
        }
        if self.iso.image_format.is_wim_container() {
            return self.prepare_wimboot();
        }

        if !self.iso.image_format.supports_virtual_disk_boot() {
            warn!(
                "Image format {} is recognized but not bootable yet: {}",
                self.iso.image_format, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let boot_config = self.boot_virtual_config();
        if let Err(err) = self.publish_os_param(&boot_config) {
            warn!(
                "Failed to publish {} for {}: {:?}",
                NEXTBOOT_OS_PARAM_NAME,
                self.iso.path,
                err.status()
            );
        }

        let virtual_device = self.create_virtual_block_io(boot_config)?;

        match self.boot_virtual_device(&virtual_device) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "Virtual device boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                );

                if !self.iso.image_format.is_iso() {
                    return Err(err);
                }

                match self.iso.os_type {
                    OsType::Windows | OsType::WinPE => self.boot_windows(&virtual_device),
                    OsType::Ubuntu
                    | OsType::Debian
                    | OsType::Fedora
                    | OsType::Arch
                    | OsType::Linux => self.boot_linux(&virtual_device),
                    OsType::Unknown => self.boot_generic(&virtual_device),
                }
            }
        }
    }

    fn boot_efi_executable(&self) -> uefi::Result<()> {
        info!("Booting selected EFI executable: {}", self.iso.path);
        let device_path = self.handle_device_path_bytes(self.iso.volume_handle)?;
        self.load_image_from_device_path(
            self.iso.volume_handle,
            &device_path,
            &self.iso.path,
            "selected EFI file",
        )
    }

    fn prepare_wimboot(&self) -> uefi::Result<()> {
        let Some(wim_info) = self.iso.wim_info else {
            warn!("WIM/ESD file has no parsed WIM metadata: {}", self.iso.path);
            return Err(Status::LOAD_ERROR.into());
        };

        if !wim_info.wimboot_supported {
            warn!(
                "WIMBOOT does not support {}: boot_index={} in_range={} compression={:?}",
                self.iso.path,
                wim_info.boot_index,
                wim_info.boot_index_in_range,
                wim_info.compression
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let helper = self.load_wimboot_helper()?;
        let inputs = self.prepare_wimboot_runtime_inputs(&helper)?;
        let runtime = self.register_wimboot_runtime_files(inputs.runtime_files)?;
        let callbacks = runtime.callbacks();
        let load_options = wimboot::build_wimboot_command_line(
            &inputs.virtual_files,
            Some(callbacks),
            Some(wim_info.boot_index),
        )
        .map_err(|_| Status::INVALID_PARAMETER)?;

        info!(
            "Prepared WIMBOOT load options for {}: {}",
            self.iso.path, load_options
        );
        info!(
            "Registered WIMBOOT runtime file callbacks: pfsize=0x{:x} pfread=0x{:x}",
            callbacks.file_size, callbacks.file_read
        );

        let source_device = VirtualBootDevice {
            handle: self.iso.volume_handle,
            device_path: self.handle_device_path_bytes(self.iso.volume_handle)?,
        };

        self.chain_load_with_options(
            &source_device,
            &helper.path,
            &helper.data,
            Some(&load_options),
        )
    }

    fn prepare_wimboot_runtime_inputs(
        &self,
        helper: &SourceVolumeFile,
    ) -> uefi::Result<WimbootRuntimeInputs> {
        let boot_wim = WimbootRuntimeFile::from_iso(self.iso, WIMBOOT_BOOT_WIM_CALLBACK_PATH)?;
        let mut internal = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
                .ok_or(uefi::Status::DEVICE_ERROR)?;
            let boot_index = self.iso.wim_info.map(|info| info.boot_index).unwrap_or(0);
            self.collect_wimboot_internal_files(&reader, &boot_wim, boot_index)
        };

        let mut runtime_files = Vec::new();
        runtime_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        runtime_files.push(boot_wim);
        runtime_files.push(WimbootRuntimeFile::from_memory(
            WIMBOOT_SELF_CALLBACK_PATH,
            helper.data.clone(),
        ));
        let mut include_bootmgfw = false;
        if let Some(bootmgfw) = internal.bootmgfw.take() {
            runtime_files.push(bootmgfw);
            include_bootmgfw = true;
        }

        let mut include_bcd = false;
        match self
            .find_source_volume_file(WIMBOOT_BCD_CANDIDATES, WIMBOOT_COMPRESSED_BCD_CANDIDATES)
        {
            Ok(mut bcd) => {
                let patched = wimboot::patch_bcd_for_efi(&mut bcd.data);
                if patched != 0 {
                    info!(
                        "Patched {} UTF-16 BCD .exe reference(s) for UEFI WIMBOOT",
                        patched
                    );
                }
                runtime_files.push(WimbootRuntimeFile::from_memory(
                    WIMBOOT_BCD_CALLBACK_PATH,
                    bcd.data,
                ));
                include_bcd = true;
            }
            Err(err) if err.status() == Status::NOT_FOUND => {
                if let Some(bcd) = internal.bcd.take() {
                    runtime_files.push(bcd);
                    include_bcd = true;
                } else {
                    info!("WIMBOOT BCD was not found externally; relying on boot.wim extraction");
                }
            }
            Err(err) => return Err(err),
        }

        let include_boot_sdi =
            match self.find_optional_source_volume_file_metadata(WIMBOOT_BOOT_SDI_CANDIDATES) {
                Ok(Some(file)) => {
                    runtime_files.push(WimbootRuntimeFile::from_source_file(
                        &file,
                        WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                    )?);
                    true
                }
                Ok(None) => {
                    if let Some(boot_sdi) = internal.boot_sdi.take() {
                        runtime_files.push(boot_sdi);
                        true
                    } else {
                        info!("WIMBOOT boot.sdi was not found; relying on boot.wim extraction");
                        false
                    }
                }
                Err(err) => return Err(err),
            };

        let mut virtual_files = Vec::new();
        virtual_files
            .try_reserve_exact(5)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        virtual_files.push(
            WimbootVirtualFile::new("boot.wim", WIMBOOT_BOOT_WIM_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        virtual_files.push(
            WimbootVirtualFile::new("vtoy_wimboot", WIMBOOT_SELF_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        if include_bootmgfw {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME,
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_bcd {
            virtual_files.push(
                WimbootVirtualFile::new("bcd", WIMBOOT_BCD_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_boot_sdi {
            virtual_files.push(
                WimbootVirtualFile::new("boot.sdi", WIMBOOT_BOOT_SDI_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }

        Ok(WimbootRuntimeInputs {
            runtime_files,
            virtual_files,
        })
    }

    fn prepare_windows_iso_wimboot(&self) -> uefi::Result<()> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        if self.iso.ventoy_windows11_bypass_check || self.iso.ventoy_windows11_bypass_nro {
            info!(
                "Windows 11 bypass controls requested for {}: hardware_check={} nro={}",
                self.iso.path,
                self.iso.ventoy_windows11_bypass_check,
                self.iso.ventoy_windows11_bypass_nro
            );
        }

        let helper = self.load_wimboot_helper()?;
        let boot_config = self.boot_virtual_config();
        let inputs = self.prepare_windows_iso_wimboot_runtime_inputs(&helper, &boot_config)?;
        let runtime = self.register_wimboot_runtime_files(inputs.runtime_files)?;
        let callbacks = runtime.callbacks();
        let load_options =
            wimboot::build_wimboot_command_line(&inputs.virtual_files, Some(callbacks), None)
                .map_err(|_| Status::INVALID_PARAMETER)?;

        info!(
            "Prepared Windows ISO WIMBOOT fallback for {}: {}",
            self.iso.path, load_options
        );

        let source_device = VirtualBootDevice {
            handle: self.iso.volume_handle,
            device_path: self.handle_device_path_bytes(self.iso.volume_handle)?,
        };

        self.chain_load_with_options(
            &source_device,
            &helper.path,
            &helper.data,
            Some(&load_options),
        )
    }

    fn prepare_windows_iso_wimboot_runtime_inputs(
        &self,
        helper: &SourceVolumeFile,
        boot_config: &VirtualDeviceConfig,
    ) -> uefi::Result<WimbootRuntimeInputs> {
        let (boot_wim, bcd, boot_sdi) = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let fs = self.open_virtual_iso_filesystem(&source_block_io)?;

            let boot_wim = self.find_iso_file_metadata(&fs, WINDOWS_ISO_BOOT_WIM_CANDIDATES)?;
            let bcd = self.find_optional_iso_file_data(&fs, WINDOWS_ISO_BCD_CANDIDATES)?;
            let boot_sdi =
                self.find_optional_iso_file_data(&fs, WINDOWS_ISO_BOOT_SDI_CANDIDATES)?;
            (boot_wim, bcd, boot_sdi)
        };
        let boot_wim_runtime = WimbootRuntimeFile::from_mapped_segments(
            WIMBOOT_BOOT_WIM_CALLBACK_PATH,
            boot_wim.size,
            boot_wim.segments,
        )?;
        let mut internal = {
            let source_block_io = self
                .bt
                .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
            let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
                .ok_or(uefi::Status::DEVICE_ERROR)?;
            self.collect_wimboot_internal_files(&reader, &boot_wim_runtime, 0)
        };

        let mut runtime_files = Vec::new();
        runtime_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        runtime_files.push(boot_wim_runtime);
        runtime_files.push(WimbootRuntimeFile::from_memory(
            WIMBOOT_SELF_CALLBACK_PATH,
            helper.data.clone(),
        ));

        let mut include_bootmgfw = false;
        if let Some(bootmgfw) = internal.bootmgfw.take() {
            runtime_files.push(bootmgfw);
            include_bootmgfw = true;
        }

        let mut include_winpeshl = false;
        if let Some(original_winpeshl) = internal.winpeshl.take() {
            match self.prepare_windows_wimboot_jump_payload(boot_config, &original_winpeshl) {
                Ok(Some(data)) => {
                    info!(
                        "Prepared Ventoy Windows WIMBOOT jump payload for {}: {} bytes",
                        self.iso.path,
                        data.len()
                    );
                    runtime_files.push(WimbootRuntimeFile::from_memory(
                        WIMBOOT_WINPESHL_CALLBACK_PATH,
                        data,
                    ));
                    include_winpeshl = true;
                }
                Ok(None) => {}
                Err(err) => warn!(
                    "Ventoy Windows WIMBOOT jump payload for {} was not prepared: {:?}",
                    self.iso.path,
                    err.status()
                ),
            }
        }

        let mut include_bcd = false;
        if let Some(mut bcd) = bcd {
            let patched = wimboot::patch_bcd_for_efi(&mut bcd.data);
            if patched != 0 {
                info!(
                    "Patched {} UTF-16 Windows ISO BCD .exe reference(s) for UEFI WIMBOOT",
                    patched
                );
            }
            runtime_files.push(WimbootRuntimeFile::from_memory(
                WIMBOOT_BCD_CALLBACK_PATH,
                bcd.data,
            ));
            include_bcd = true;
        } else if let Some(bcd) = internal.bcd.take() {
            runtime_files.push(bcd);
            include_bcd = true;
        } else {
            info!("Windows ISO BCD was not found externally; relying on boot.wim extraction");
        }

        let mut include_boot_sdi = false;
        if let Some(boot_sdi) = boot_sdi {
            runtime_files.push(WimbootRuntimeFile::from_memory(
                WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                boot_sdi.data,
            ));
            include_boot_sdi = true;
        } else if let Some(boot_sdi) = internal.boot_sdi.take() {
            runtime_files.push(boot_sdi);
            include_boot_sdi = true;
        } else {
            info!("Windows ISO boot.sdi was not found externally; relying on boot.wim extraction");
        }

        let mut virtual_files = Vec::new();
        virtual_files
            .try_reserve_exact(6)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        virtual_files.push(
            WimbootVirtualFile::new("boot.wim", WIMBOOT_BOOT_WIM_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        virtual_files.push(
            WimbootVirtualFile::new("vtoy_wimboot", WIMBOOT_SELF_CALLBACK_PATH)
                .map_err(|_| Status::INVALID_PARAMETER)?,
        );
        if include_bootmgfw {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME,
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_winpeshl {
            virtual_files.push(
                WimbootVirtualFile::new(
                    WIMBOOT_WINPESHL_VIRTUAL_NAME,
                    WIMBOOT_WINPESHL_CALLBACK_PATH,
                )
                .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_bcd {
            virtual_files.push(
                WimbootVirtualFile::new("bcd", WIMBOOT_BCD_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }
        if include_boot_sdi {
            virtual_files.push(
                WimbootVirtualFile::new("boot.sdi", WIMBOOT_BOOT_SDI_CALLBACK_PATH)
                    .map_err(|_| Status::INVALID_PARAMETER)?,
            );
        }

        Ok(WimbootRuntimeInputs {
            runtime_files,
            virtual_files,
        })
    }

    fn prepare_windows_wimboot_jump_payload(
        &self,
        boot_config: &VirtualDeviceConfig,
        original_winpeshl: &[u8],
    ) -> uefi::Result<Option<Vec<u8>>> {
        let jump = match self.find_source_volume_file(VTOYJUMP_CANDIDATES, &[]) {
            Ok(file) => file,
            Err(err) if err.status() == Status::NOT_FOUND => {
                info!(
                    "Ventoy Windows jump helper was not found for {}; skipping winpeshl.exe overlay",
                    self.iso.path
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

        let plugin = self.iso.ventoy_plugin.as_ref();
        let auto_install = self.load_selected_auto_install_template(plugin)?;
        let injection = self.load_plugin_injection_archive(plugin)?;
        let runtime_input = nextboot_windows::VentoyWindowsRuntimeDataInput {
            auto_install: auto_install.as_ref().map(|file| {
                nextboot_windows::VentoyWindowsAutoInstall {
                    source_path: file.path.as_str(),
                    data: file.data.as_slice(),
                }
            }),
            injection_archive: injection.as_ref().map(|file| file.path.as_str()),
            windows11_bypass_check: self.iso.ventoy_windows11_bypass_check,
            windows11_bypass_nro: self.iso.ventoy_windows11_bypass_nro,
        };
        let windows_data = nextboot_windows::build_ventoy_windows_runtime_data(runtime_input)
            .map_err(ventoy_windows_runtime_data_error_to_uefi_status)?;
        let (os_param, image_chunks, image_location_addr) =
            self.build_ventoy_os_param_payload(boot_config)?;
        let payload = nextboot_windows::build_ventoy_wimboot_jump_payload(
            jump.data.as_slice(),
            &os_param,
            windows_data.as_slice(),
            original_winpeshl,
        )
        .map_err(ventoy_windows_wimboot_payload_error_to_uefi_status)?;

        info!(
            "Built Ventoy Windows runtime data for {}: jump={}, auto_install={}, injection={}, win11_bypass_check={}, win11_bypass_nro={}, image_chunks={}, image_location=0x{:x}",
            self.iso.path,
            jump.path,
            auto_install.is_some(),
            injection.is_some(),
            self.iso.ventoy_windows11_bypass_check,
            self.iso.ventoy_windows11_bypass_nro,
            image_chunks,
            image_location_addr
        );

        Ok(Some(payload))
    }

    fn collect_wimboot_internal_files(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        boot_index: u32,
    ) -> WimbootInternalFiles {
        let image = match self.load_wimboot_wim_image(reader, boot_wim, boot_index) {
            Ok(image) => image,
            Err(err) => {
                warn!(
                    "Could not inspect WIM internals for {}: {:?}",
                    self.iso.path,
                    err.status()
                );
                return WimbootInternalFiles::default();
            }
        };

        let bootmgfw = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BOOTMGFW_CANDIDATES)
            .map(|resource| {
                info!(
                    "Registered WIM internal {} ({} bytes)",
                    WIMBOOT_BOOTMGFW_VIRTUAL_NAME, resource.uncompressed_size
                );
                WimbootRuntimeFile::from_wim_resource(
                    WIMBOOT_BOOTMGFW_CALLBACK_PATH,
                    boot_wim,
                    image.metadata,
                    resource,
                )
            });

        let boot_sdi = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BOOT_SDI_CANDIDATES)
            .map(|resource| {
                info!(
                    "Registered WIM internal boot.sdi ({} bytes)",
                    resource.uncompressed_size
                );
                WimbootRuntimeFile::from_wim_resource(
                    WIMBOOT_BOOT_SDI_CALLBACK_PATH,
                    boot_wim,
                    image.metadata,
                    resource,
                )
            });

        let bcd = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_BCD_CANDIDATES)
            .and_then(|resource| {
                match self.read_wim_resource_to_vec(reader, boot_wim, &image.metadata, &resource) {
                    Ok(mut data) => {
                        let patched = wimboot::patch_bcd_for_efi(&mut data);
                        if patched != 0 {
                            info!(
                                "Patched {} UTF-16 WIM internal BCD .exe reference(s) for UEFI WIMBOOT",
                                patched
                            );
                        }
                        info!("Registered WIM internal BCD ({} bytes)", data.len());
                        Some(WimbootRuntimeFile::from_memory(WIMBOOT_BCD_CALLBACK_PATH, data))
                    }
                    Err(err) => {
                        warn!(
                            "Could not read WIM internal BCD for {}: {:?}",
                            self.iso.path,
                            err.status()
                        );
                        None
                    }
                }
            });

        let winpeshl = self
            .find_wim_resource_file(&image, WIMBOOT_WIM_WINPESHL_CANDIDATES)
            .and_then(|resource| {
                match self.read_wim_resource_to_vec(reader, boot_wim, &image.metadata, &resource) {
                    Ok(data) => {
                        info!("Loaded WIM internal winpeshl.exe ({} bytes)", data.len());
                        Some(data)
                    }
                    Err(err) => {
                        warn!(
                            "Could not read WIM internal winpeshl.exe for {}: {:?}",
                            self.iso.path,
                            err.status()
                        );
                        None
                    }
                }
            });

        WimbootInternalFiles {
            bootmgfw,
            bcd,
            boot_sdi,
            winpeshl,
        }
    }

    fn load_wimboot_wim_image(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        boot_index: u32,
    ) -> uefi::Result<WimbootWimImage> {
        let mut header = [0u8; wim::WIM_HEADER_SIZE];
        boot_wim
            .read_range(reader, 0, &mut header)
            .ok_or(Status::DEVICE_ERROR)?;
        let metadata = wim::parse_wim_metadata(&header).ok_or(Status::LOAD_ERROR)?;
        if !metadata.is_wimboot_supported() {
            return Err(Status::UNSUPPORTED.into());
        }

        let lookup =
            self.read_wim_resource_to_vec(reader, boot_wim, &metadata, &metadata.lookup)?;
        let image_index = boot_index;
        let image_metadata_resource =
            wim::metadata_resource_for_image(&metadata, &lookup, image_index)
                .ok_or(Status::NOT_FOUND)?;
        let image_metadata =
            self.read_wim_resource_to_vec(reader, boot_wim, &metadata, &image_metadata_resource)?;

        Ok(WimbootWimImage {
            metadata,
            lookup,
            image_metadata,
        })
    }

    fn read_wim_resource_to_vec(
        &self,
        reader: &SourceVolumeReader,
        boot_wim: &WimbootRuntimeFile,
        metadata: &wim::WimMetadata,
        resource: &wim::WimResourceHeader,
    ) -> uefi::Result<Vec<u8>> {
        let len =
            usize::try_from(resource.uncompressed_size).map_err(|_| Status::OUT_OF_RESOURCES)?;
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        out.resize(len, 0);
        wim::read_resource_range_with(
            metadata,
            boot_wim.size,
            resource,
            0,
            &mut out,
            |offset, buf| {
                boot_wim
                    .read_range(reader, offset, buf)
                    .ok_or(wim::WimReadError::ResourceOutOfBounds)
            },
        )
        .map_err(wim_read_error_to_uefi_status)?;
        Ok(out)
    }

    fn find_wim_resource_file(
        &self,
        image: &WimbootWimImage,
        candidates: &[&str],
    ) -> Option<wim::WimResourceHeader> {
        for path in candidates {
            match wim::file_resource_for_path(&image.image_metadata, &image.lookup, path) {
                Ok(resource) => return Some(resource),
                Err(wim::WimPathError::NotFound | wim::WimPathError::ResourceNotFound) => {}
                Err(err) => {
                    warn!(
                        "WIM internal file candidate {} failed for {}: {:?}",
                        path, self.iso.path, err
                    );
                }
            }
        }

        None
    }

    fn register_wimboot_runtime_files(
        &self,
        files: Vec<WimbootRuntimeFile>,
    ) -> uefi::Result<WimbootRuntimeRegistration<'a>> {
        let bt: &'a BootServices = self.bt;
        let source_block_io = bt.open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;

        Ok(WimbootRuntimeRegistration::install(
            WimbootRuntimeContext { reader, files },
            source_block_io,
        ))
    }

    fn load_wimboot_helper(&self) -> uefi::Result<SourceVolumeFile> {
        let candidates = wimboot_helper_candidates();
        if candidates.is_empty() {
            warn!("WIMBOOT EFI helper is not available for this firmware architecture");
            return Err(Status::UNSUPPORTED.into());
        }

        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded WIMBOOT helper {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "WIMBOOT helper candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        for path in compressed_wimboot_helper_candidates() {
            match self.load_compressed_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded compressed WIMBOOT helper {} -> {} bytes",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Compressed WIMBOOT helper candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }

    /// 引导 Linux ISO
    fn boot_linux(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        use nextboot_linux::{EfiStubOptions, LinuxBootloader, LinuxDistro};

        info!("Booting Linux ISO...");
        if let Ok(()) = self.try_chain_load_paths(device, generic_efi_boot_paths()) {
            return Ok(());
        }

        // 映射发行版类型
        let distro = match self.iso.os_type {
            OsType::Ubuntu => LinuxDistro::Ubuntu,
            OsType::Debian => LinuxDistro::Debian,
            OsType::Fedora => LinuxDistro::Fedora,
            OsType::Arch => LinuxDistro::Arch,
            _ => LinuxDistro::Generic,
        };

        // 创建启动配置
        let config = self.discover_linux_boot_config(distro)?;

        info!("Kernel: {}", config.kernel_path);
        info!("Initrd: {}", config.initrd_path);
        info!("Cmdline: {}", config.cmdline);

        // 创建启动器
        let mut bootloader = LinuxBootloader::new(config);

        // 加载 Kernel
        let kernel_data = self.load_file(&bootloader.config().kernel_path)?;
        bootloader
            .load_kernel(kernel_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        // 加载 Initrd
        let mut initrd_data = self.load_file(&bootloader.config().initrd_path)?;
        if let Err(err) = self.append_ventoy_linux_initrd_overlay(&mut initrd_data) {
            warn!(
                "Failed to append Ventoy Linux initrd overlay for {}: {:?}",
                self.iso.path,
                err.status()
            );
        }
        bootloader
            .load_initrd(initrd_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        let kernel_size = bootloader.kernel_size();
        let initrd_size = bootloader.initrd_size();
        let (config, kernel_data, initrd_data) = bootloader.into_parts();
        drop(kernel_data);

        let load_options =
            EfiStubOptions::new(&config.cmdline, &config.initrd_path).to_load_option_string();
        info!(
            "Prepared Linux EFI stub: kernel={} bytes initrd={} bytes options={}",
            kernel_size, initrd_size, load_options
        );

        match LinuxInitrdLoadFile2Protocol::install(self.bt, initrd_data) {
            Ok(provider) => {
                info!(
                    "Registered Linux EFI initrd LoadFile2 provider: {} bytes",
                    initrd_size
                );
                provider.leak();
            }
            Err(err) => warn!(
                "Failed to register Linux EFI initrd LoadFile2 provider: {:?}; falling back to initrd path load option",
                err.status()
            ),
        }

        self.load_image_from_device_path_with_options(
            device.handle,
            &device.device_path,
            &config.kernel_path,
            "Linux EFI stub",
            Some(&load_options),
        )
    }

    fn discover_linux_boot_config(
        &self,
        distro: nextboot_linux::LinuxDistro,
    ) -> uefi::Result<nextboot_linux::LinuxBootConfig> {
        if let Some(config) = self.discover_linux_config_file_boot_config(distro)? {
            return Ok(config);
        }

        if let Some(config) = self.discover_linux_candidate_boot_config(distro)? {
            return Ok(config);
        }

        let config = nextboot_linux::LinuxBootConfig::for_distro(distro, &self.iso.path);
        warn!(
            "Falling back to built-in Linux boot paths for {}: kernel={} initrd={}",
            self.iso.path, config.kernel_path, config.initrd_path
        );
        Ok(config)
    }

    fn discover_linux_config_file_boot_config(
        &self,
        distro: nextboot_linux::LinuxDistro,
    ) -> uefi::Result<Option<nextboot_linux::LinuxBootConfig>> {
        let mut config_paths = Vec::new();
        for path in LINUX_GRUB_CONFIG_CANDIDATES
            .iter()
            .chain(LINUX_ISOLINUX_CONFIG_CANDIDATES.iter())
        {
            push_unique_iso_path(&mut config_paths, path)?;
        }
        self.discover_linux_loader_entry_configs(&mut config_paths)?;

        for path in config_paths {
            let text = match self.load_iso_text_file(&path, LINUX_CONFIG_MAX_SIZE) {
                Ok(text) => text,
                Err(err) if err.status() == Status::NOT_FOUND => continue,
                Err(err) => {
                    warn!(
                        "Linux config candidate {} was not loaded: {:?}",
                        path,
                        err.status()
                    );
                    continue;
                }
            };

            let parsed = if is_isolinux_config_path(&path) {
                nextboot_linux::parse_isolinux_cfg(&text)
                    .or_else(|| nextboot_linux::parse_grub_cfg(&text))
            } else {
                nextboot_linux::parse_grub_cfg(&text)
                    .or_else(|| nextboot_linux::parse_isolinux_cfg(&text))
            };
            let Some((kernel, initrd, cmdline)) = parsed else {
                info!(
                    "Linux config {} did not contain a complete boot entry",
                    path
                );
                continue;
            };

            let base_dir = iso_parent_dir(&path);
            let kernel_path = resolve_linux_config_path(&base_dir, &kernel);
            let initrd_path = resolve_linux_config_path(&base_dir, &initrd);
            if !self.iso_file_exists(&kernel_path)? {
                warn!(
                    "Linux config {} references missing kernel {}",
                    path, kernel_path
                );
                continue;
            }
            if !self.iso_file_exists(&initrd_path)? {
                warn!(
                    "Linux config {} references missing initrd {}",
                    path, initrd_path
                );
                continue;
            }

            info!(
                "Discovered Linux boot config from {}: kernel={} initrd={} cmdline={}",
                path, kernel_path, initrd_path, cmdline
            );
            return Ok(Some(nextboot_linux::LinuxBootConfig::from_paths(
                distro,
                &self.iso.path,
                &kernel_path,
                &initrd_path,
                &cmdline,
            )));
        }

        Ok(None)
    }

    fn discover_linux_loader_entry_configs(&self, paths: &mut Vec<String>) -> uefi::Result<()> {
        for dir in LINUX_LOADER_ENTRY_DIRS {
            let entries = match self.read_iso_dir(dir) {
                Ok(entries) => entries,
                Err(err) if err.status() == Status::NOT_FOUND => continue,
                Err(err) => {
                    warn!(
                        "Linux loader entry dir {} was not scanned: {:?}",
                        dir,
                        err.status()
                    );
                    continue;
                }
            };

            for entry in entries {
                if entry.is_dir || !entry.name.to_ascii_lowercase().ends_with(".conf") {
                    continue;
                }

                push_unique_iso_path(paths, &format!("{}/{}", dir, entry.name))?;
            }
        }

        Ok(())
    }

    fn discover_linux_candidate_boot_config(
        &self,
        distro: nextboot_linux::LinuxDistro,
    ) -> uefi::Result<Option<nextboot_linux::LinuxBootConfig>> {
        let default = nextboot_linux::LinuxBootConfig::for_distro(distro, &self.iso.path);
        if self.iso_file_exists(&default.kernel_path)?
            && self.iso_file_exists(&default.initrd_path)?
        {
            info!(
                "Using distro Linux defaults: kernel={} initrd={}",
                default.kernel_path, default.initrd_path
            );
            return Ok(Some(default));
        }

        let Some(kernel_path) = self.first_existing_iso_file(LINUX_KERNEL_CANDIDATES)? else {
            return Ok(None);
        };
        let Some(initrd_path) = self.first_existing_iso_file(LINUX_INITRD_CANDIDATES)? else {
            return Ok(None);
        };

        info!(
            "Using Ventoy-style Linux candidates: kernel={} initrd={}",
            kernel_path, initrd_path
        );
        Ok(Some(nextboot_linux::LinuxBootConfig::from_paths(
            distro,
            &self.iso.path,
            &kernel_path,
            &initrd_path,
            &default.cmdline,
        )))
    }

    fn append_ventoy_linux_initrd_overlay(&self, initrd_data: &mut Vec<u8>) -> uefi::Result<()> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        let boot_config = self.boot_virtual_config();
        let (os_param, _, _) = self.build_ventoy_os_param_payload(&boot_config)?;
        let image_map = self.build_ventoy_linux_image_map()?;

        let mut base_archives = Vec::new();
        self.try_load_ventoy_cpio_archives(&mut base_archives)?;
        let base_refs: Vec<&[u8]> = base_archives
            .iter()
            .map(|file: &SourceVolumeFile| file.data.as_slice())
            .collect();

        let plugin = self.iso.ventoy_plugin.as_ref();
        let auto_install = self.load_selected_auto_install_template(plugin)?;
        let persistent_map = self.load_selected_persistence_map(plugin)?;
        let injection = self.load_plugin_injection_archive(plugin)?;
        let dud_files = self.load_plugin_dud_files(plugin)?;
        let dud_refs: Vec<VentoyDudFile<'_>> = dud_files
            .iter()
            .map(|file| VentoyDudFile {
                source_path: file.path.as_str(),
                data: file.data.as_slice(),
            })
            .collect();

        let input = VentoyLinuxInitrdInput {
            base_archives: &base_refs,
            image_map: &image_map,
            os_param: &os_param,
            auto_install: auto_install.as_ref().map(|file| file.data.as_slice()),
            persistent_map: persistent_map.as_deref(),
            injection_archive: injection.as_ref().map(|file| file.data.as_slice()),
            dud_files: &dud_refs,
        };
        let overlay = crate::ventoy_linux::build_ventoy_linux_initrd(&input)
            .map_err(ventoy_linux_error_to_uefi_status)?;

        initrd_data
            .try_reserve_exact(overlay.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        initrd_data.extend_from_slice(&overlay);

        info!(
            "Appended Ventoy Linux initrd overlay: {} bytes, base_archives={}, image_chunks={}, auto_install={}, persistence={}, injection={}, dud_files={}",
            overlay.len(),
            base_archives.len(),
            image_map.len(),
            auto_install.is_some(),
            persistent_map.as_ref().map_or(0, Vec::len),
            injection.is_some(),
            dud_refs.len()
        );

        Ok(())
    }

    fn build_ventoy_linux_image_map(
        &self,
    ) -> uefi::Result<Vec<crate::ventoy_linux::VentoyImageMapChunk>> {
        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(self.iso.block_size, |disk| disk.block_size);
        if disk_sector_size != self.iso.block_size {
            warn!(
                "Ventoy Linux initrd map source disk sector size {} differs from volume sector size {} for {}",
                disk_sector_size, self.iso.block_size, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let extents = self.ventoy_source_extents()?;
        crate::ventoy_linux::build_image_map_chunks(&extents, self.iso.block_size, 2048)
            .map_err(ventoy_linux_error_to_uefi_status)
            .map_err(Into::into)
    }

    fn try_load_ventoy_cpio_archives(
        &self,
        archives: &mut Vec<SourceVolumeFile>,
    ) -> uefi::Result<()> {
        for path in VENTOY_COMMON_CPIO_CANDIDATES
            .iter()
            .chain(ventoy_arch_cpio_candidates().iter())
        {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded Ventoy cpio archive {} ({} bytes)",
                        path,
                        file.data.len()
                    );
                    archives
                        .try_reserve_exact(1)
                        .map_err(|_| Status::OUT_OF_RESOURCES)?;
                    archives.push(file);
                }
                Err(err) => {
                    info!(
                        "Ventoy cpio archive {} not loaded: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Ok(())
    }

    fn load_selected_auto_install_template(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let Some(auto_install) = plugin.and_then(|plugin| plugin.auto_install.as_ref()) else {
            return Ok(None);
        };
        let Some(index) =
            selected_ventoy_plugin_index(auto_install.autosel, auto_install.templates.len())
        else {
            return Ok(None);
        };
        let Some(path) = auto_install.templates.get(index) else {
            return Ok(None);
        };

        match self.load_source_volume_file(path) {
            Ok(file) => Ok(Some(file)),
            Err(err) => {
                warn!(
                    "Ventoy auto_install template {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                Ok(None)
            }
        }
    }

    fn load_selected_persistence_map(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<Vec<crate::ventoy_linux::VentoyImageMapChunk>>> {
        let Some(persistence) = plugin.and_then(|plugin| plugin.persistence.as_ref()) else {
            return Ok(None);
        };
        let Some(index) =
            selected_persistence_backend_index(persistence.autosel, persistence.backends.len())
        else {
            info!(
                "Ventoy persistence is configured for {}, but no backend is selected",
                self.iso.path
            );
            return Ok(None);
        };
        let Some(path) = persistence.backends.get(index) else {
            return Ok(None);
        };

        let metadata = match self.source_volume_file_metadata(path) {
            Ok(metadata) => metadata,
            Err(err) => {
                warn!(
                    "Ventoy persistence backend {} for {} was not mapped: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                return Ok(None);
            }
        };

        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(metadata.block_size, |disk| disk.block_size);
        if disk_sector_size != metadata.block_size {
            warn!(
                "Ventoy persistence backend {} source disk sector size {} differs from volume sector size {}",
                metadata.path, disk_sector_size, metadata.block_size
            );
            return Ok(None);
        }

        let extents = self.ventoy_source_volume_extents(&metadata.extents)?;
        let chunks =
            match crate::ventoy_linux::build_image_map_chunks(&extents, metadata.block_size, 512) {
                Ok(chunks) if !chunks.is_empty() => chunks,
                Ok(_) => {
                    warn!(
                        "Ventoy persistence backend {} for {} has no mapped extents",
                        metadata.path, self.iso.path
                    );
                    return Ok(None);
                }
                Err(err) => {
                    warn!(
                        "Ventoy persistence backend {} for {} has unsupported extents: {:?}",
                        metadata.path, self.iso.path, err
                    );
                    return Ok(None);
                }
            };

        info!(
            "Mapped Ventoy persistence backend {} for {}: {} chunks, block_size={}",
            metadata.path,
            self.iso.path,
            chunks.len(),
            metadata.block_size
        );
        Ok(Some(chunks))
    }

    fn ventoy_source_volume_extents(
        &self,
        source_extents: &[IsoExtent],
    ) -> uefi::Result<Vec<crate::ventoy::VentoyExtent>> {
        let mut extents = Vec::new();
        extents
            .try_reserve_exact(source_extents.len())
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        let disk_lba_offset = self
            .iso
            .source_disk
            .map_or(0, |disk| disk.partition_start_lba);
        for extent in source_extents {
            extents.push(crate::ventoy::VentoyExtent {
                virtual_block_start: extent.virtual_block_start,
                physical_lba: extent
                    .physical_lba
                    .checked_add(disk_lba_offset)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                block_count: extent.block_count,
            });
        }

        Ok(extents)
    }

    fn load_plugin_injection_archive(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let Some(path) = plugin.and_then(|plugin| plugin.injection_archive.as_deref()) else {
            return Ok(None);
        };

        match self.load_source_volume_file(path) {
            Ok(file) => Ok(Some(file)),
            Err(err) => {
                warn!(
                    "Ventoy injection archive {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                );
                Ok(None)
            }
        }
    }

    fn load_plugin_dud_files(
        &self,
        plugin: Option<&crate::ventoy_config::VentoyImagePlugin>,
    ) -> uefi::Result<Vec<SourceVolumeFile>> {
        let mut files = Vec::new();
        let Some(dud) = plugin.and_then(|plugin| plugin.dud.as_ref()) else {
            return Ok(files);
        };

        files
            .try_reserve_exact(dud.files.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        for path in &dud.files {
            match self.load_source_volume_file(path) {
                Ok(file) => files.push(file),
                Err(err) => warn!(
                    "Ventoy DUD file {} for {} was not loaded: {:?}",
                    path,
                    self.iso.path,
                    err.status()
                ),
            }
        }

        Ok(files)
    }

    /// 引导 Windows ISO
    fn boot_windows(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Booting Windows ISO...");
        match self.chain_load_path(device, WINDOWS_BOOTMGFW_PATH) {
            Ok(()) => return Ok(()),
            Err(err) => warn!(
                "Windows Boot Manager chain-load failed with {:?}; trying default EFI paths",
                err.status()
            ),
        }

        match self.try_chain_load_paths(device, generic_efi_boot_paths()) {
            Ok(()) => Ok(()),
            Err(chain_err) => {
                warn!(
                    "Windows default EFI chain-load paths failed with {:?}; trying WIMBOOT fallback",
                    chain_err.status()
                );
                match self.prepare_windows_iso_wimboot() {
                    Ok(()) => Ok(()),
                    Err(wimboot_err) => {
                        warn!(
                            "Windows ISO WIMBOOT fallback failed with {:?}",
                            wimboot_err.status()
                        );
                        Err(chain_err)
                    }
                }
            }
        }
    }

    /// 通用引导 (尝试链式加载)
    fn boot_generic(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Attempting generic boot...");
        self.try_chain_load_paths(device, generic_efi_boot_paths())
    }

    fn try_chain_load_paths(&self, device: &VirtualBootDevice, paths: &[&str]) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;

        for path in paths {
            match self.chain_load_path(device, path) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_status = err.status();
                    warn!("Chain-load path {} failed with {:?}", path, err.status());
                }
            }
        }

        Err(last_status.into())
    }

    fn try_load_image_paths(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        paths: &[&str],
        label: &str,
    ) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;

        for path in paths {
            match self.load_image_from_device_path(device_handle, device_path, path, label) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "LoadImage path {} on {} failed with {:?}",
                        path,
                        label,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }

    fn chain_load_path(&self, device: &VirtualBootDevice, path: &str) -> uefi::Result<()> {
        let data = self.load_file(path)?;
        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        self.chain_load_with_options(device, path, &data, None)
    }

    /// 链式加载 EFI 文件
    fn chain_load_with_options(
        &self,
        device: &VirtualBootDevice,
        path: &str,
        data: &[u8],
        load_options: Option<&str>,
    ) -> uefi::Result<()> {
        info!("Chain loading: {} ({} bytes)", path, data.len());

        if data.is_empty() {
            return Err(Status::LOAD_ERROR.into());
        }

        let full_path = append_file_path_device_path(&device.device_path, path)
            .ok_or(Status::INVALID_PARAMETER)?;
        let file_path =
            unsafe { DevicePath::from_ffi_ptr(full_path.as_ptr().cast::<FfiDevicePath>()) };

        let image = self.bt.load_image(
            self.parent_image,
            LoadImageSource::FromBuffer {
                buffer: data,
                file_path: Some(file_path),
            },
        )?;

        let load_options = match load_options {
            Some(options) => Some(LoadOptionsBuffer::new(options)?),
            None => None,
        };
        if let Err(err) = self.patch_loaded_image(
            image,
            device.handle,
            full_path.as_ptr().cast::<FfiDevicePath>(),
            load_options.as_ref(),
        ) {
            warn!(
                "Failed to rebind LoadedImage source/options for {}: {:?}",
                path,
                err.status()
            );
        }

        info!("Loaded chained EFI image {:?} from {}", image, path);
        match self.bt.start_image(image) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "StartImage failed for chained image {}: {:?}",
                    path,
                    err.status()
                );
                let _ = self.bt.unload_image(image);
                Err(err)
            }
        }
    }

    fn load_image_from_device_path(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        path: &str,
        label: &str,
    ) -> uefi::Result<()> {
        self.load_image_from_device_path_with_options(device_handle, device_path, path, label, None)
    }

    fn load_image_from_device_path_with_options(
        &self,
        device_handle: Handle,
        device_path: &[u8],
        path: &str,
        label: &str,
        load_options: Option<&str>,
    ) -> uefi::Result<()> {
        let full_path =
            append_file_path_device_path(device_path, path).ok_or(Status::INVALID_PARAMETER)?;
        let full_device_path =
            unsafe { DevicePath::from_ffi_ptr(full_path.as_ptr().cast::<FfiDevicePath>()) };

        info!("Trying {} EFI loader path: {}", label, path);
        let image = self.bt.load_image(
            self.parent_image,
            LoadImageSource::FromDevicePath {
                device_path: full_device_path,
                from_boot_manager: true,
            },
        )?;

        let load_options = match load_options {
            Some(options) => Some(LoadOptionsBuffer::new(options)?),
            None => None,
        };
        if let Err(err) = self.patch_loaded_image(
            image,
            device_handle,
            full_path.as_ptr().cast::<FfiDevicePath>(),
            load_options.as_ref(),
        ) {
            warn!(
                "Failed to rebind LoadedImage source/options for {} on {}: {:?}",
                path,
                label,
                err.status()
            );
        }

        info!("Loaded EFI image {:?} from {} path {}", image, label, path);
        match self.bt.start_image(image) {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    "StartImage failed for {} on {} with {:?}",
                    path,
                    label,
                    err.status()
                );
                let _ = self.bt.unload_image(image);
                Err(err)
            }
        }
    }

    fn patch_loaded_image(
        &self,
        image: Handle,
        source_device: Handle,
        file_path: *const FfiDevicePath,
        load_options: Option<&LoadOptionsBuffer>,
    ) -> uefi::Result<()> {
        let mut loaded_image = self.bt.open_protocol_exclusive::<RawLoadedImage>(image)?;
        loaded_image.0.device_handle = source_device.as_ptr();
        loaded_image.0.file_path = file_path;
        if let Some(load_options) = load_options {
            loaded_image.0.load_options_size = load_options.size_bytes();
            loaded_image.0.load_options = load_options.as_ptr();
        } else {
            loaded_image.0.load_options_size = 0;
            loaded_image.0.load_options = ptr::null();
        }
        Ok(())
    }

    /// 从 ISO 加载文件
    fn load_file(&self, path: &str) -> uefi::Result<Vec<u8>> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        info!("Loading file: {}", path);

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;

        let file = self.load_iso_file_from_fs(&fs, path)?;
        info!(
            "Loaded {} bytes from ISO path {}",
            file.data.len(),
            file.path
        );

        Ok(file.data)
    }

    fn load_iso_text_file(&self, path: &str, max_size: usize) -> uefi::Result<String> {
        let data = self.load_file(path)?;
        if data.len() > max_size {
            return Err(Status::OUT_OF_RESOURCES.into());
        }

        let text = core::str::from_utf8(&data).map_err(|_| Status::LOAD_ERROR)?;
        Ok(String::from(text))
    }

    fn read_iso_dir(&self, path: &str) -> uefi::Result<Vec<nextboot_fs::FileInfo>> {
        if !self.iso.image_format.is_iso() {
            return Err(Status::UNSUPPORTED.into());
        }

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;
        fs.read_dir(&normalize_iso_path(path))
            .map_err(fs_error_to_uefi_status)
            .map_err(Into::into)
    }

    fn iso_file_exists(&self, path: &str) -> uefi::Result<bool> {
        if !self.iso.image_format.is_iso() {
            return Ok(false);
        }

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let fs = self.open_virtual_iso_filesystem(&source_block_io)?;
        match fs.stat(&normalize_iso_path(path)) {
            Ok(info) => Ok(!info.is_dir),
            Err(FsError::FileNotFound | FsError::DirectoryNotFound) => Ok(false),
            Err(err) => Err(fs_error_to_uefi_status(err).into()),
        }
    }

    fn first_existing_iso_file(&self, candidates: &[&str]) -> uefi::Result<Option<String>> {
        for path in candidates {
            if self.iso_file_exists(path)? {
                return Ok(Some(normalize_iso_path(path)));
            }
        }

        Ok(None)
    }

    fn find_iso_file_metadata(
        &self,
        fs: &VirtualIsoFilesystem,
        candidates: &[&str],
    ) -> uefi::Result<IsoMappedFileMetadata> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.iso_file_metadata(fs, path) {
                Ok(file) => {
                    info!(
                        "Found ISO file {} ({} bytes, {} mapped segment(s))",
                        file.path,
                        file.size,
                        file.segments.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!("ISO file candidate {} failed: {:?}", path, err.status());
                }
            }
        }

        Err(last_status.into())
    }

    fn find_optional_iso_file_data(
        &self,
        fs: &VirtualIsoFilesystem,
        candidates: &[&str],
    ) -> uefi::Result<Option<SourceVolumeFile>> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_iso_file_from_fs(fs, path) {
                Ok(file) => {
                    info!(
                        "Loaded optional ISO file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(Some(file));
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Optional ISO file candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        if last_status == Status::NOT_FOUND {
            Ok(None)
        } else {
            Err(last_status.into())
        }
    }

    fn load_iso_file_from_fs(
        &self,
        fs: &VirtualIsoFilesystem,
        path: &str,
    ) -> uefi::Result<SourceVolumeFile> {
        let path = normalize_iso_path(path);
        let info = fs.stat(&path).map_err(fs_error_to_uefi_status)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let file_size = usize::try_from(info.size).map_err(|_| Status::OUT_OF_RESOURCES)?;
        let mut data = Vec::new();
        data.try_reserve_exact(file_size)
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        data.resize(file_size, 0);

        let read = fs
            .read_file(&path, 0, &mut data)
            .map_err(fs_error_to_uefi_status)?;
        data.truncate(read);
        Ok(SourceVolumeFile { path, data })
    }

    fn iso_file_metadata(
        &self,
        fs: &VirtualIsoFilesystem,
        path: &str,
    ) -> uefi::Result<IsoMappedFileMetadata> {
        let path = normalize_iso_path(path);
        let info = fs.stat(&path).map_err(fs_error_to_uefi_status)?;
        if info.is_dir {
            return Err(Status::UNSUPPORTED.into());
        }

        let file_extents = fs.file_extents(&path).map_err(fs_error_to_uefi_status)?;
        let segments = self.map_iso_file_extents_to_source_segments(
            fs.block_size(),
            info.size,
            &file_extents,
        )?;

        Ok(IsoMappedFileMetadata {
            path,
            size: info.size,
            segments,
        })
    }

    fn load_source_volume_file(&self, path: &str) -> uefi::Result<SourceVolumeFile> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.asset_volume_handle)?;
        let fs = SourceVolumeFileSystem::open(&source_block_io, self.iso.asset_source_disk)?;
        fs.load_file(path)
    }

    fn load_compressed_source_volume_file(&self, path: &str) -> uefi::Result<SourceVolumeFile> {
        let compressed = self.load_source_volume_file(path)?;
        let data = xz::decompress_xz(&compressed.data, WIMBOOT_XZ_MAX_OUTPUT_SIZE)
            .map_err(xz_error_to_uefi_status)?;
        let mut decompressed_path = compressed.path.clone();
        if decompressed_path.ends_with(".xz") {
            decompressed_path.truncate(decompressed_path.len() - 3);
        }

        Ok(SourceVolumeFile {
            path: decompressed_path,
            data,
        })
    }

    fn find_source_volume_file(
        &self,
        candidates: &[&str],
        compressed_candidates: &[&str],
    ) -> uefi::Result<SourceVolumeFile> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.load_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded source volume file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!("Source volume file {} failed: {:?}", path, err.status());
                }
            }
        }

        for path in compressed_candidates {
            match self.load_compressed_source_volume_file(path) {
                Ok(file) => {
                    info!(
                        "Loaded compressed source volume file {} ({} bytes)",
                        file.path,
                        file.data.len()
                    );
                    return Ok(file);
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Compressed source volume file {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        Err(last_status.into())
    }

    fn source_volume_file_metadata(&self, path: &str) -> uefi::Result<SourceVolumeFileMetadata> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.asset_volume_handle)?;
        let fs = SourceVolumeFileSystem::open(&source_block_io, self.iso.asset_source_disk)?;
        fs.file_metadata(path)
    }

    fn find_optional_source_volume_file_metadata(
        &self,
        candidates: &[&str],
    ) -> uefi::Result<Option<SourceVolumeFileMetadata>> {
        let mut last_status = Status::NOT_FOUND;
        for path in candidates {
            match self.source_volume_file_metadata(path) {
                Ok(file) => {
                    info!(
                        "Found optional source volume file {} ({} bytes)",
                        file.path, file.size
                    );
                    return Ok(Some(file));
                }
                Err(err) if err.status() == Status::NOT_FOUND => {
                    last_status = Status::NOT_FOUND;
                }
                Err(err) => {
                    last_status = err.status();
                    warn!(
                        "Optional source volume candidate {} failed: {:?}",
                        path,
                        err.status()
                    );
                }
            }
        }

        if last_status == Status::NOT_FOUND {
            Ok(None)
        } else {
            Err(last_status.into())
        }
    }

    /// 创建虚拟 Block IO
    fn create_virtual_block_io(
        &self,
        mut config: VirtualDeviceConfig,
    ) -> uefi::Result<VirtualBootDevice> {
        use nextboot_virtio::protocol::VirtualBlockIoProtocol;

        info!("Creating virtual Block IO...");
        let load_file_entries = self.preload_load_file_entries();

        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let memory_overlays = self.build_conf_replace_overlays(&source_block_io, &mut config)?;
        let efi_file_replacements = self.build_efi_file_replacements(&source_block_io)?;
        let mut vbio = self.build_virtual_block_io(config, &source_block_io)?;
        for overlay in memory_overlays {
            vbio.add_memory_overlay(overlay)
                .map_err(virtio_error_to_uefi_status)?;
        }
        let virtual_info = vbio.device_info();
        let registered = VirtualBlockIoProtocol::new(vbio).install(self.bt)?;
        let virtual_handle = registered.handle();
        let device_path = registered.device_path().to_vec();

        let simple_file_system = if self.iso.image_format.is_iso() {
            match self.install_iso_simple_file_system(
                &source_block_io,
                virtual_handle,
                efi_file_replacements,
            ) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install SimpleFileSystem on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        } else {
            None
        };

        let load_file_protocol = if load_file_entries.is_empty() {
            None
        } else {
            match PreloadedLoadFileProtocol::install(self.bt, virtual_handle, load_file_entries) {
                Ok(protocol) => Some(protocol),
                Err(err) => {
                    warn!(
                        "Failed to install LoadFile protocols on virtual device {:?}: {:?}",
                        virtual_handle,
                        err.status()
                    );
                    None
                }
            }
        };

        registered.leak();
        if let Some(protocol) = simple_file_system {
            protocol.leak();
        }
        if let Some(protocol) = load_file_protocol {
            protocol.leak();
        }

        info!(
            "Virtual Block IO installed on {:?}: {:?}, source extents: {}",
            virtual_handle,
            virtual_info,
            self.iso.extents.len()
        );

        Ok(VirtualBootDevice {
            handle: virtual_handle,
            device_path,
        })
    }

    fn build_conf_replace_overlays(
        &self,
        source_block_io: &BlockIO,
        config: &mut VirtualDeviceConfig,
    ) -> uefi::Result<Vec<MemoryOverlay>> {
        let Some(plugin) = self.iso.ventoy_plugin.as_ref() else {
            return Ok(Vec::new());
        };
        if plugin.conf_replace.is_empty() {
            return Ok(Vec::new());
        }
        if !self.iso.image_format.is_iso() {
            return Ok(Vec::new());
        }
        let source_fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        let mut overlays = Vec::new();
        overlays
            .try_reserve_exact(plugin.conf_replace.len().saturating_mul(3))
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        let mut next_append_offset =
            align_up_u64(config.iso_size, ISO9660_SECTOR_SIZE).ok_or(Status::OUT_OF_RESOURCES)?;

        if self.iso.is_udf {
            let udf_fs = self.open_udf_filesystem(source_block_io)?;
            for rule in &plugin.conf_replace {
                let replacement = match source_fs.load_file(&rule.new_path) {
                    Ok(file) => file,
                    Err(err) => {
                        warn!(
                            "Ventoy UDF conf_replace new path {} for {} was not loaded: {:?}",
                            rule.new_path,
                            self.iso.path,
                            err.status()
                        );
                        continue;
                    }
                };
                if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                    warn!(
                        "Ventoy UDF conf_replace new path {} for {} is too large: {} bytes",
                        replacement.path,
                        self.iso.path,
                        replacement.data.len()
                    );
                    continue;
                }

                let aligned_len = align_up(replacement.data.len(), ISO9660_SECTOR_SIZE as usize)
                    .ok_or(Status::OUT_OF_RESOURCES)?;
                let replacement_size =
                    u64::try_from(replacement.data.len()).map_err(|_| Status::OUT_OF_RESOURCES)?;
                let replacement_sector = next_append_offset / ISO9660_SECTOR_SIZE;

                let patch = match udf_fs.file_replacement_patch(
                    &rule.org,
                    replacement_sector,
                    replacement_size,
                    aligned_len as u64,
                ) {
                    Ok(patch) => patch,
                    Err(err) => {
                        warn!(
                            "Ventoy UDF conf_replace org path {} for {} was not patched: {:?}",
                            rule.org, self.iso.path, err
                        );
                        continue;
                    }
                };

                overlays.push(MemoryOverlay::new(
                    patch.file_entry_offset,
                    patch.file_entry_data,
                ));
                if let Some(partition_descriptor) = patch.partition_descriptor {
                    overlays.push(MemoryOverlay::new(
                        partition_descriptor.descriptor_offset,
                        partition_descriptor.descriptor_data,
                    ));
                }

                let mut data = replacement.data;
                data.resize(aligned_len, 0);
                overlays.push(MemoryOverlay::new(next_append_offset, data));

                info!(
                    "Prepared Ventoy UDF conf_replace for {}: {} -> {} at virtual sector {} ({} bytes)",
                    self.iso.path, rule.org, replacement.path, replacement_sector, replacement_size
                );
                next_append_offset = next_append_offset
                    .checked_add(aligned_len as u64)
                    .ok_or(Status::OUT_OF_RESOURCES)?;
            }

            if !overlays.is_empty() {
                config.iso_size = config.iso_size.max(next_append_offset);
                info!(
                    "Prepared {} Ventoy UDF conf_replace overlay(s) for {}; virtual size now {} bytes",
                    overlays.len(),
                    self.iso.path,
                    config.iso_size
                );
            }

            return Ok(overlays);
        }

        let iso_fs = self.open_iso9660_filesystem(source_block_io)?;
        for rule in &plugin.conf_replace {
            let record = match iso_fs.directory_record_location(&rule.org) {
                Ok(record) if !record.is_dir => record,
                Ok(_) => {
                    warn!(
                        "Ventoy conf_replace org path {} for {} is a directory",
                        rule.org, self.iso.path
                    );
                    continue;
                }
                Err(err) => {
                    warn!(
                        "Ventoy conf_replace org path {} for {} was not found: {:?}",
                        rule.org, self.iso.path, err
                    );
                    continue;
                }
            };

            let replacement = match source_fs.load_file(&rule.new_path) {
                Ok(file) => file,
                Err(err) => {
                    warn!(
                        "Ventoy conf_replace new path {} for {} was not loaded: {:?}",
                        rule.new_path,
                        self.iso.path,
                        err.status()
                    );
                    continue;
                }
            };
            if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                warn!(
                    "Ventoy conf_replace new path {} for {} is too large: {} bytes",
                    replacement.path,
                    self.iso.path,
                    replacement.data.len()
                );
                continue;
            }

            let aligned_len = align_up(replacement.data.len(), ISO9660_SECTOR_SIZE as usize)
                .ok_or(Status::OUT_OF_RESOURCES)?;
            let replacement_size =
                u32::try_from(replacement.data.len()).map_err(|_| Status::OUT_OF_RESOURCES)?;
            let replacement_sector = u32::try_from(next_append_offset / ISO9660_SECTOR_SIZE)
                .map_err(|_| Status::OUT_OF_RESOURCES)?;
            let patch_offset = record
                .record_offset
                .checked_add(2)
                .ok_or(Status::OUT_OF_RESOURCES)?;

            overlays.push(MemoryOverlay::new(
                patch_offset,
                iso9660_file_extent_patch(replacement_sector, replacement_size),
            ));

            let mut data = replacement.data;
            data.resize(aligned_len, 0);
            overlays.push(MemoryOverlay::new(next_append_offset, data));

            info!(
                "Prepared Ventoy conf_replace for {}: {} -> {} at virtual sector {} ({} bytes)",
                self.iso.path, rule.org, replacement.path, replacement_sector, replacement_size
            );
            next_append_offset = next_append_offset
                .checked_add(aligned_len as u64)
                .ok_or(Status::OUT_OF_RESOURCES)?;
        }

        if !overlays.is_empty() {
            config.iso_size = config.iso_size.max(next_append_offset);
            info!(
                "Prepared {} Ventoy conf_replace overlay(s) for {}; virtual size now {} bytes",
                overlays.len() / 2,
                self.iso.path,
                config.iso_size
            );
        }

        Ok(overlays)
    }

    fn build_efi_file_replacements(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<Vec<VirtualFileReplacement>> {
        let Some(plugin) = self.iso.ventoy_plugin.as_ref() else {
            return Ok(Vec::new());
        };
        if plugin.conf_replace.is_empty() || !self.iso.image_format.is_iso() {
            return Ok(Vec::new());
        }

        let img_replace_count = plugin
            .conf_replace
            .iter()
            .filter(|rule| rule.img.unwrap_or(0) > 0)
            .count();
        if img_replace_count == 0 {
            return Ok(Vec::new());
        }

        let source_fs = SourceVolumeFileSystem::open(source_block_io, self.iso.source_disk)?;
        let mut replacements = Vec::new();
        replacements
            .try_reserve_exact(img_replace_count)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        for rule in plugin
            .conf_replace
            .iter()
            .filter(|rule| rule.img.unwrap_or(0) > 0)
        {
            let replacement = match source_fs.load_file(&rule.new_path) {
                Ok(file) => file,
                Err(err) => {
                    warn!(
                        "Ventoy EFI img_replace new path {} for {} was not loaded: {:?}",
                        rule.new_path,
                        self.iso.path,
                        err.status()
                    );
                    continue;
                }
            };
            if replacement.data.len() > VENTOY_CONF_REPLACE_MAX_SIZE {
                warn!(
                    "Ventoy EFI img_replace new path {} for {} is too large: {} bytes",
                    replacement.path,
                    self.iso.path,
                    replacement.data.len()
                );
                continue;
            }

            info!(
                "Prepared Ventoy EFI img_replace for {}: {} -> {} ({} bytes)",
                self.iso.path,
                rule.org,
                replacement.path,
                replacement.data.len()
            );
            replacements.push(VirtualFileReplacement::new(&rule.org, replacement.data));
        }

        Ok(replacements)
    }

    fn open_iso9660_filesystem(&self, source_block_io: &BlockIO) -> uefi::Result<Iso9660> {
        let config = self.iso9660_virtual_config();
        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        Ok(
            Iso9660::open(Rc::new(VirtualIsoBlockIo::new(vbio)))
                .map_err(fs_error_to_uefi_status)?,
        )
    }

    fn open_udf_filesystem(&self, source_block_io: &BlockIO) -> uefi::Result<Udf> {
        let config = self.iso9660_virtual_config();
        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        Ok(Udf::open(Rc::new(VirtualIsoBlockIo::new(vbio))).map_err(fs_error_to_uefi_status)?)
    }

    fn publish_os_param(&self, config: &VirtualDeviceConfig) -> uefi::Result<()> {
        let data = self.build_os_param_payload(config)?;
        let name = CString16::try_from(NEXTBOOT_OS_PARAM_NAME)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        let vendor = VariableVendor(NEXTBOOT_OS_PARAM_VENDOR_GUID);
        let attributes =
            VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.rt
            .set_variable(name.as_ref(), &vendor, attributes, &data)?;
        info!(
            "Published {} ({} bytes, {} extent record(s))",
            NEXTBOOT_OS_PARAM_NAME,
            data.len(),
            runtime_extent_count(self.iso)
        );

        if let Err(err) = self.publish_ventoy_os_param(config) {
            warn!(
                "Failed to publish {} for {}: {:?}",
                crate::ventoy::VENTOY_OS_PARAM_NAME,
                self.iso.path,
                err.status()
            );
        }

        Ok(())
    }

    fn publish_ventoy_os_param(&self, config: &VirtualDeviceConfig) -> uefi::Result<()> {
        let (data, image_region_count, image_location_addr) =
            self.build_ventoy_os_param_payload(config)?;
        let name = CString16::try_from(crate::ventoy::VENTOY_OS_PARAM_NAME)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        let vendor = VariableVendor(VENTOY_OS_PARAM_VENDOR_GUID);
        let attributes =
            VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.rt
            .set_variable(name.as_ref(), &vendor, attributes, &data)?;
        info!(
            "Published {} ({} bytes, {} image location region(s), location=0x{:x})",
            crate::ventoy::VENTOY_OS_PARAM_NAME,
            data.len(),
            image_region_count,
            image_location_addr
        );

        Ok(())
    }

    fn build_ventoy_os_param_payload(
        &self,
        config: &VirtualDeviceConfig,
    ) -> uefi::Result<([u8; crate::ventoy::VENTOY_OS_PARAM_SIZE], usize, usize)> {
        let (image_sector_size, disk_sector_size, image_regions) =
            self.build_ventoy_image_regions(config)?;
        let image_location = crate::ventoy::build_ventoy_image_location(
            image_sector_size,
            disk_sector_size,
            &image_regions,
        )
        .map_err(ventoy_error_to_uefi_status)?;
        let image_location_addr =
            self.copy_to_runtime_pool_aligned(&image_location, VENTOY_RUNTIME_ALIGNMENT)?;
        let source_disk = self.iso.source_disk;
        let disk_part_type = self
            .detect_ventoy_source_partition_type()
            .unwrap_or(crate::ventoy::VENTOY_PART_TYPE_OTHER);
        let disk_part_id = source_disk
            .and_then(|disk| {
                if disk.partition_number == 0 {
                    None
                } else {
                    Some(disk.partition_number)
                }
            })
            .unwrap_or(usize_to_u16(self.iso.volume_index.saturating_add(1))?);
        let disk_signature = source_disk.map_or([0; 4], |disk| disk.disk_signature);
        let reserved = self.ventoy_reserved_flags(disk_signature);
        let input = crate::ventoy::VentoyOsParamInput {
            disk_guid: source_disk.map_or([0; 16], |disk| disk.disk_guid),
            disk_size: source_disk.map_or(self.iso.source_disk_size, |disk| disk.disk_size),
            disk_part_id,
            disk_part_type,
            image_path: &self.iso.path,
            image_size: self.iso.size,
            image_location_addr: image_location_addr as u64,
            image_location_len: usize_to_u32(image_location.len())?,
            reserved,
            disk_signature,
        };
        let data =
            crate::ventoy::build_ventoy_os_param(&input).map_err(ventoy_error_to_uefi_status)?;
        Ok((data, image_regions.len(), image_location_addr))
    }

    fn ventoy_reserved_flags(&self, disk_signature: [u8; 4]) -> crate::ventoy::VentoyReserved {
        let chain_type = ventoy_chain_type(self.iso);
        let windows_cd_prompt =
            chain_type == crate::ventoy::VENTOY_CHAIN_WINDOWS && self.iso.ventoy_windows_cd_prompt;
        let windows_resolution_lock = if chain_type == crate::ventoy::VENTOY_CHAIN_WINDOWS {
            self.iso.ventoy_windows_uefi_resolution_lock
        } else {
            0
        };

        crate::ventoy::VentoyReserved::new()
            .with_chain_type(chain_type)
            .with_iso_udf(self.iso.is_udf)
            .with_windows_cd_prompt(windows_cd_prompt)
            .with_linux_remount(self.iso.ventoy_linux_remount)
            .with_vlnk(self.iso.is_vlnk)
            .with_disk_signature(disk_signature)
            .with_windows_max_resolution(windows_resolution_lock)
    }

    fn build_ventoy_image_regions(
        &self,
        config: &VirtualDeviceConfig,
    ) -> uefi::Result<(u32, u32, Vec<crate::ventoy::VentoyImageRegion>)> {
        let disk_sector_size = self
            .iso
            .source_disk
            .map_or(self.iso.block_size, |disk| disk.block_size);
        if disk_sector_size != self.iso.block_size {
            warn!(
                "VentoyOsParam source disk sector size {} differs from volume sector size {} for {}",
                disk_sector_size, self.iso.block_size, self.iso.path
            );
            return Err(Status::UNSUPPORTED.into());
        }

        let extents = self.ventoy_source_extents()?;
        let preferred_image_sector_size = if self.iso.image_format.is_iso() {
            2048
        } else {
            config.block_size
        };

        match crate::ventoy::build_ventoy_image_regions(
            &extents,
            self.iso.block_size,
            preferred_image_sector_size,
        ) {
            Ok(regions) => Ok((preferred_image_sector_size, disk_sector_size, regions)),
            Err(crate::ventoy::VentoyParamError::UnalignedExtent)
                if preferred_image_sector_size != self.iso.block_size =>
            {
                let regions = crate::ventoy::build_ventoy_image_regions(
                    &extents,
                    self.iso.block_size,
                    self.iso.block_size,
                )
                .map_err(ventoy_error_to_uefi_status)?;
                Ok((self.iso.block_size, disk_sector_size, regions))
            }
            Err(err) => Err(ventoy_error_to_uefi_status(err).into()),
        }
    }

    fn ventoy_source_extents(&self) -> uefi::Result<Vec<crate::ventoy::VentoyExtent>> {
        let mut extents = Vec::new();
        let count = if self.iso.extents.is_empty() {
            1
        } else {
            self.iso.extents.len()
        };
        extents
            .try_reserve_exact(count)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        if self.iso.extents.is_empty() {
            let disk_lba_offset = self
                .iso
                .source_disk
                .map_or(0, |disk| disk.partition_start_lba);
            let block_count = div_round_up(self.iso.size, u64::from(self.iso.block_size))
                .ok_or(uefi::Status::INVALID_PARAMETER)?;
            extents.push(crate::ventoy::VentoyExtent {
                virtual_block_start: 0,
                physical_lba: self
                    .iso
                    .start_lba
                    .checked_add(disk_lba_offset)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                block_count,
            });
        } else {
            let disk_lba_offset = self
                .iso
                .source_disk
                .map_or(0, |disk| disk.partition_start_lba);
            for extent in &self.iso.extents {
                extents.push(crate::ventoy::VentoyExtent {
                    virtual_block_start: extent.virtual_block_start,
                    physical_lba: extent
                        .physical_lba
                        .checked_add(disk_lba_offset)
                        .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
                    block_count: extent.block_count,
                });
            }
        }

        Ok(extents)
    }

    fn copy_to_runtime_pool_aligned(&self, data: &[u8], alignment: usize) -> uefi::Result<usize> {
        if data.is_empty() || !alignment.is_power_of_two() {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let allocation_size = data
            .len()
            .checked_add(
                alignment
                    .checked_mul(2)
                    .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
            )
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let raw = self
            .bt
            .allocate_pool(MemoryType::RUNTIME_SERVICES_DATA, allocation_size)?;
        let aligned = align_up(raw as usize, alignment).ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), aligned as *mut u8, data.len());
        }

        Ok(aligned)
    }

    fn detect_ventoy_source_partition_type(&self) -> uefi::Result<u16> {
        let source_block_io = self
            .bt
            .open_protocol_exclusive::<BlockIO>(self.iso.volume_handle)?;
        let reader = SourceVolumeReader::new(&source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        let shared: SharedBlockIo = Rc::new(reader);
        let block_size =
            usize::try_from(shared.block_size()).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
        if block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let mut boot_sector = Vec::new();
        boot_sector
            .try_reserve_exact(block_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        boot_sector.resize(block_size, 0);
        shared
            .read_blocks(0, &mut boot_sector)
            .map_err(fs_error_to_uefi_status)?;

        let fs_type = detect_fs_type(&boot_sector);
        let source_is_udf = matches!(fs_type, FileSystemType::Unknown | FileSystemType::Iso9660)
            && Udf::open(shared).is_ok();

        Ok(match fs_type {
            FileSystemType::ExFat => crate::ventoy::VENTOY_PART_TYPE_EXFAT,
            FileSystemType::Fat32 => crate::ventoy::VENTOY_PART_TYPE_FAT,
            FileSystemType::Ntfs => crate::ventoy::VENTOY_PART_TYPE_NTFS,
            _ if source_is_udf => crate::ventoy::VENTOY_PART_TYPE_UDF,
            _ => crate::ventoy::VENTOY_PART_TYPE_OTHER,
        })
    }

    fn build_os_param_payload(&self, config: &VirtualDeviceConfig) -> uefi::Result<Vec<u8>> {
        let path = self.iso.path.as_bytes();
        let extent_count = runtime_extent_count(self.iso);
        let path_offset = NEXTBOOT_OS_PARAM_HEADER_SIZE;
        let path_end = path_offset
            .checked_add(path.len())
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_offset = align_up(path_end, 8).ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let extents_len = extent_count
            .checked_mul(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;
        let total_size = extents_offset
            .checked_add(extents_len)
            .ok_or(uefi::Status::OUT_OF_RESOURCES)?;

        let mut flags = 0u16;
        if self.iso.extents.is_empty() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_SYNTHETIC_EXTENT;
        }
        if self.iso.boot_info.is_some() {
            flags |= NEXTBOOT_OS_PARAM_FLAG_EL_TORITO;
        }

        let mut data = Vec::new();
        data.try_reserve_exact(total_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.extend_from_slice(NEXTBOOT_OS_PARAM_MAGIC);
        push_u16(&mut data, NEXTBOOT_OS_PARAM_VERSION);
        push_u16(&mut data, usize_to_u16(NEXTBOOT_OS_PARAM_HEADER_SIZE)?);
        push_u16(
            &mut data,
            usize_to_u16(NEXTBOOT_OS_PARAM_EXTENT_RECORD_SIZE)?,
        );
        push_u16(&mut data, flags);
        push_u32(&mut data, usize_to_u32(total_size)?);
        push_u32(&mut data, usize_to_u32(self.iso.volume_index)?);
        push_u32(&mut data, os_type_code(self.iso.os_type));
        push_u32(&mut data, virtual_device_type_code(config.device_type));
        push_u64(&mut data, self.iso.virtual_size);
        push_u64(&mut data, self.iso.start_lba);
        push_u32(&mut data, self.iso.block_size);
        push_u32(&mut data, config.block_size);
        push_u32(&mut data, config.physical_block_size);
        push_u32(&mut data, usize_to_u32(extent_count)?);
        push_u32(&mut data, usize_to_u32(path_offset)?);
        push_u32(&mut data, usize_to_u32(path.len())?);
        push_u32(&mut data, usize_to_u32(extents_offset)?);
        push_u32(&mut data, usize_to_u32(extents_len)?);
        debug_assert_eq!(data.len(), NEXTBOOT_OS_PARAM_HEADER_SIZE);

        data.extend_from_slice(path);
        data.resize(extents_offset, 0);
        self.append_runtime_extents(&mut data)?;
        debug_assert_eq!(data.len(), total_size);

        Ok(data)
    }

    fn append_runtime_extents(&self, data: &mut Vec<u8>) -> uefi::Result<()> {
        if self.iso.extents.is_empty() {
            let block_count = div_round_up(self.iso.virtual_size, u64::from(self.iso.block_size))
                .ok_or(uefi::Status::INVALID_PARAMETER)?;
            push_extent_record(data, 0, self.iso.start_lba, block_count);
            return Ok(());
        }

        for extent in &self.iso.extents {
            push_extent_record(
                data,
                extent.virtual_block_start,
                extent.physical_lba,
                extent.block_count,
            );
        }

        Ok(())
    }

    fn boot_virtual_config(&self) -> VirtualDeviceConfig {
        use nextboot_virtio::CdRomBootInfo;

        let device_type = if self.iso.image_format.is_iso() {
            match self.iso.os_type {
                OsType::Windows | OsType::WinPE => VirtualDeviceType::DvdRom,
                _ => VirtualDeviceType::HardDisk,
            }
        } else {
            VirtualDeviceType::HardDisk
        };
        let virtual_block_size = if let Some(block_size) = self.iso.virtual_block_size {
            block_size
        } else if self.iso.image_format.uses_512_byte_virtual_sectors() {
            512
        } else {
            match device_type {
                VirtualDeviceType::DvdRom => 2048,
                _ => self.iso.block_size,
            }
        };

        let mut config = VirtualDeviceConfig::new(
            device_type,
            self.iso.start_lba,
            self.iso.virtual_size,
            virtual_block_size,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        if let Some(boot) = self.iso.boot_info {
            config = config.with_cdrom_boot(CdRomBootInfo::new(
                boot.boot_entry,
                u64::from(boot.image_lba),
                boot.image_block_count,
            ));
            info!(
                "Using EFI El Torito boot image: catalog LBA {}, entry {}, image LBA {}, blocks {}",
                boot.catalog_lba, boot.boot_entry, boot.image_lba, boot.image_block_count
            );
        } else if self.iso.image_format.is_iso() && matches!(device_type, VirtualDeviceType::DvdRom)
        {
            warn!("No EFI El Torito boot image found for {}", self.iso.path);
        }

        config
    }

    fn iso9660_virtual_config(&self) -> VirtualDeviceConfig {
        VirtualDeviceConfig::new(
            VirtualDeviceType::DvdRom,
            self.iso.start_lba,
            self.iso.size,
            2048,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path)
    }

    fn build_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        if self
            .iso
            .ventoy_plugin
            .as_ref()
            .is_some_and(|plugin| plugin.auto_memdisk)
        {
            return self.build_auto_memdisk_block_io(config, source_block_io);
        }

        self.build_source_backed_virtual_block_io(config, source_block_io)
    }

    fn build_source_backed_virtual_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let mut vbio = if self.iso.image_format == ImageFormat::DynamicVhd {
            self.build_dynamic_vhd_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vhdx {
            self.build_vhdx_block_io(config, source_block_io)?
        } else if self.iso.image_format == ImageFormat::Vdi {
            self.build_vdi_block_io(config, source_block_io)?
        } else if self.iso.extents.is_empty() {
            warn!(
                "No extent map for {}, falling back to contiguous LBA {}",
                self.iso.path, self.iso.start_lba
            );
            VirtualBlockIo::new(config)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = SourceVolumeReader::new(source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);

        Ok(vbio)
    }

    fn build_auto_memdisk_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let image_size =
            usize::try_from(config.iso_size).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut image = Vec::new();
        image
            .try_reserve_exact(image_size)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        image.resize(image_size, 0);

        let source_vbio =
            self.build_source_backed_virtual_block_io(config.clone(), source_block_io)?;
        vhd::read_file_bytes(&source_vbio, 0, &mut image)?;

        info!(
            "Using Ventoy auto_memdisk for {} ({} bytes loaded)",
            self.iso.path, image_size
        );

        let mut vbio = VirtualBlockIo::new(config);
        vbio.set_physical_reader(ZeroPhysicalReader);
        vbio.add_memory_overlay(MemoryOverlay::new(0, image))
            .map_err(virtio_error_to_uefi_status)?;
        Ok(vbio)
    }

    fn build_dynamic_vhd_block_io(
        &self,
        config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let mut footer = [0u8; vhd::FOOTER_SIZE];
        let footer_offset = self
            .iso
            .size
            .checked_sub(vhd::FOOTER_SIZE as u64)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        vhd::read_file_bytes(&file_vbio, footer_offset, &mut footer)?;

        let footer = vhd::parse_dynamic_footer(&footer).ok_or(uefi::Status::LOAD_ERROR)?;
        let virtual_size = config.iso_size;
        if footer.virtual_size != virtual_size {
            warn!(
                "Dynamic VHD virtual size mismatch for {}: scanner={} footer={}",
                self.iso.path, virtual_size, footer.virtual_size
            );
        }

        let mut header = alloc::vec![0u8; vhd::DYNAMIC_HEADER_SIZE];
        vhd::read_file_bytes(&file_vbio, footer.data_offset, &mut header)?;
        let header = vhd::parse_dynamic_header(&header).ok_or(uefi::Status::LOAD_ERROR)?;
        if header.header_version != 0x0001_0000 {
            warn!(
                "Dynamic VHD header version for {} is 0x{:08x}",
                self.iso.path, header.header_version
            );
        }

        let block_size = u64::from(header.block_size);
        if virtual_size == 0 || block_size == 0 || block_size % vhd::SECTOR_SIZE != 0 {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let sectors_per_block = block_size / vhd::SECTOR_SIZE;
        let bitmap_bytes = div_round_up(sectors_per_block, 8)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let entries_needed =
            div_round_up(virtual_size, block_size).ok_or(uefi::Status::LOAD_ERROR)?;
        if entries_needed == 0 || u64::from(header.max_table_entries) < entries_needed {
            return Err(uefi::Status::LOAD_ERROR.into());
        }
        let entries_to_scan = entries_needed;

        let bat_bytes = entries_to_scan
            .checked_mul(4)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        vhd::read_file_bytes(&file_vbio, header.table_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;

        for index in 0..entries_to_scan {
            let bat_offset = usize::try_from(index.checked_mul(4).ok_or(uefi::Status::LOAD_ERROR)?)
                .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let bat_entry = vhd::read_be_u32(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            if bat_entry == vhd::UNUSED_BAT_ENTRY {
                continue;
            }

            let virtual_start = index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= virtual_size {
                break;
            }
            let byte_count = block_size.min(virtual_size - virtual_start);
            let file_offset = u64::from(bat_entry)
                .checked_mul(vhd::SECTOR_SIZE)
                .and_then(|offset| offset.checked_add(bitmap_bytes))
                .ok_or(uefi::Status::LOAD_ERROR)?;

            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(virtual_size);
        byte_mapping.optimize();
        info!(
            "Mapped dynamic VHD {}: virtual={} bytes, block={} bytes, allocated BAT entries={}, physical segments={}",
            self.iso.path,
            virtual_size,
            block_size,
            allocated_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vhdx_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let (regions, metadata) = self.read_vhdx_layout(&file_vbio)?;
        if metadata.has_parent {
            warn!(
                "VHDX parent chains are not supported yet: {}",
                self.iso.path
            );
            return Err(uefi::Status::UNSUPPORTED.into());
        }

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.logical_sector_size;

        let block_size = u64::from(metadata.block_size);
        let payload_blocks =
            vhdx::payload_block_count(metadata.virtual_disk_size, metadata.block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
        let chunk_ratio = metadata.chunk_ratio().ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_entries =
            vhdx::bat_entry_count(payload_blocks, chunk_ratio).ok_or(uefi::Status::LOAD_ERROR)?;
        let bat_bytes = bat_entries
            .checked_mul(8)
            .and_then(|bytes| align_up_u64(bytes, vhd::SECTOR_SIZE))
            .ok_or(uefi::Status::LOAD_ERROR)?;

        if bat_bytes > regions.bat_length {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let bat_len = usize::try_from(bat_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut bat = Vec::new();
        bat.try_reserve_exact(bat_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        bat.resize(bat_len, 0);
        vhd::read_file_bytes(&file_vbio, regions.bat_offset, &mut bat)?;

        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for payload_index in 0..payload_blocks {
            let bat_index = vhdx::payload_bat_index(payload_index, chunk_ratio)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let bat_offset =
                usize::try_from(bat_index.checked_mul(8).ok_or(uefi::Status::LOAD_ERROR)?)
                    .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
            let raw_entry = vhdx::read_le_u64(&bat, bat_offset).ok_or(uefi::Status::LOAD_ERROR)?;
            let entry = vhdx::parse_bat_entry(raw_entry);
            let virtual_start = payload_index
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);

            match entry.state {
                vhdx::VHDX_BAT_STATE_FULLY_PRESENT => {
                    if entry
                        .file_offset
                        .checked_add(byte_count)
                        .map_or(true, |end| end > self.iso.size)
                    {
                        return Err(uefi::Status::DEVICE_ERROR.into());
                    }

                    self.map_image_file_range_to_physical(
                        &mut byte_mapping,
                        virtual_start,
                        entry.file_offset,
                        byte_count,
                    )?;
                    allocated_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_NOT_PRESENT
                | vhdx::VHDX_BAT_STATE_ZERO
                | vhdx::VHDX_BAT_STATE_UNMAPPED => {
                    zero_blocks += 1;
                }
                vhdx::VHDX_BAT_STATE_UNDEFINED | vhdx::VHDX_BAT_STATE_PARTIALLY_PRESENT => {
                    warn!(
                        "Unsupported VHDX BAT state {} at payload block {} in {}",
                        entry.state, payload_index, self.iso.path
                    );
                    return Err(uefi::Status::UNSUPPORTED.into());
                }
                _ => {
                    return Err(uefi::Status::LOAD_ERROR.into());
                }
            }
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VHDX {}: virtual={} bytes, block={} bytes, logical_sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.logical_sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn build_vdi_block_io(
        &self,
        mut config: VirtualDeviceConfig,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualBlockIo> {
        let file_vbio = self.build_image_file_block_io(source_block_io)?;
        let metadata = self.read_vdi_metadata(&file_vbio)?;

        config.iso_size = metadata.virtual_disk_size;
        config.block_size = metadata.sector_size;

        let map_bytes =
            vdi::block_map_bytes(metadata.block_count).ok_or(uefi::Status::LOAD_ERROR)?;
        if metadata
            .offset_blocks
            .checked_add(map_bytes)
            .map_or(true, |end| {
                end > self.iso.size || end > metadata.offset_data
            })
        {
            return Err(uefi::Status::LOAD_ERROR.into());
        }

        let map_len = usize::try_from(map_bytes).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut block_map = Vec::new();
        block_map
            .try_reserve_exact(map_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        block_map.resize(map_len, 0);
        vhd::read_file_bytes(&file_vbio, metadata.offset_blocks, &mut block_map)?;

        let block_size = u64::from(metadata.block_size);
        let mut byte_mapping = ByteMappingTable::empty();
        let mut allocated_blocks = 0u64;
        let mut zero_blocks = 0u64;

        for block_index in 0..metadata.block_count {
            let virtual_start = u64::from(block_index)
                .checked_mul(block_size)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if virtual_start >= metadata.virtual_disk_size {
                break;
            }

            let byte_count = block_size.min(metadata.virtual_disk_size - virtual_start);
            let map_entry = vdi::read_block_map_entry(&block_map, block_index)
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if !vdi::is_allocated_block(map_entry) {
                zero_blocks += 1;
                continue;
            }

            if map_entry >= metadata.block_count {
                warn!(
                    "Invalid VDI block map entry {} at virtual block {} in {}",
                    map_entry, block_index, self.iso.path
                );
                return Err(uefi::Status::LOAD_ERROR.into());
            }

            let file_offset = metadata
                .offset_data
                .checked_add(
                    u64::from(map_entry)
                        .checked_mul(block_size)
                        .ok_or(uefi::Status::LOAD_ERROR)?,
                )
                .ok_or(uefi::Status::LOAD_ERROR)?;
            if file_offset
                .checked_add(byte_count)
                .map_or(true, |end| end > self.iso.size)
            {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }

            self.map_image_file_range_to_physical(
                &mut byte_mapping,
                virtual_start,
                file_offset,
                byte_count,
            )?;
            allocated_blocks += 1;
        }

        byte_mapping.truncate(metadata.virtual_disk_size);
        byte_mapping.optimize();
        info!(
            "Mapped VDI {}: virtual={} bytes, block={} bytes, sector={} bytes, allocated_blocks={}, zero_blocks={}, physical_segments={}",
            self.iso.path,
            metadata.virtual_disk_size,
            block_size,
            metadata.sector_size,
            allocated_blocks,
            zero_blocks,
            byte_mapping.segment_count()
        );

        Ok(VirtualBlockIo::with_byte_mapping(config, byte_mapping))
    }

    fn read_vhdx_layout(
        &self,
        file_vbio: &VirtualBlockIo,
    ) -> uefi::Result<(vhdx::VhdxRegions, vhdx::VhdxMetadata)> {
        let mut header = Vec::new();
        header
            .try_reserve_exact(vhdx::VHDX_HEADER_SECTION_SIZE)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        header.resize(vhdx::VHDX_HEADER_SECTION_SIZE, 0);
        vhd::read_file_bytes(file_vbio, 0, &mut header)?;
        let regions = vhdx::parse_vhdx_regions(&header).ok_or(uefi::Status::LOAD_ERROR)?;

        let metadata_len =
            usize::try_from(regions.metadata_length).map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        let mut metadata = Vec::new();
        metadata
            .try_reserve_exact(metadata_len)
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        metadata.resize(metadata_len, 0);
        vhd::read_file_bytes(file_vbio, regions.metadata_offset, &mut metadata)?;
        let metadata = vhdx::parse_vhdx_metadata(&metadata).ok_or(uefi::Status::LOAD_ERROR)?;

        Ok((regions, metadata))
    }

    fn read_vdi_metadata(&self, file_vbio: &VirtualBlockIo) -> uefi::Result<vdi::VdiMetadata> {
        let mut header = [0u8; vdi::VDI_HEADER_SIZE];
        vhd::read_file_bytes(file_vbio, 0, &mut header)?;
        vdi::parse_vdi_metadata(&header).ok_or(uefi::Status::LOAD_ERROR.into())
    }

    fn build_image_file_block_io(&self, source_block_io: &BlockIO) -> uefi::Result<VirtualBlockIo> {
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::HardDisk,
            self.iso.start_lba,
            self.iso.size,
            vhd::SECTOR_SIZE as u32,
        )
        .with_physical_block_size(self.iso.block_size)
        .with_name(&self.iso.path);

        let mut vbio = if self.iso.extents.is_empty() {
            let source_block_size = u64::from(self.iso.block_size);
            let block_count =
                div_round_up(self.iso.size, source_block_size).ok_or(uefi::Status::LOAD_ERROR)?;
            let extents = [(0, self.iso.start_lba, block_count)];
            VirtualBlockIo::from_file_extents(config, &extents)
        } else {
            let extents: Vec<(u64, u64, u64)> = self
                .iso
                .extents
                .iter()
                .map(|extent| {
                    (
                        extent.virtual_block_start,
                        extent.physical_lba,
                        extent.block_count,
                    )
                })
                .collect();
            VirtualBlockIo::from_file_extents(config, &extents)
        };

        let reader = SourceVolumeReader::new(source_block_io, self.iso.source_disk)
            .ok_or(uefi::Status::DEVICE_ERROR)?;
        vbio.set_physical_reader(reader);
        Ok(vbio)
    }

    fn map_iso_file_extents_to_source_segments(
        &self,
        iso_block_size: u32,
        file_size: u64,
        extents: &[FileExtent],
    ) -> uefi::Result<Vec<WimbootMappedSegment>> {
        if iso_block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }
        if file_size == 0 {
            return Ok(Vec::new());
        }
        if extents.is_empty() {
            return Err(Status::UNSUPPORTED.into());
        }

        let iso_block_size = u64::from(iso_block_size);
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(extents.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;

        for extent in extents {
            let file_virtual_start = extent
                .virtual_block_start
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;
            if file_virtual_start >= file_size {
                continue;
            }

            let extent_bytes = extent
                .block_count
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;
            let byte_count = extent_bytes.min(file_size - file_virtual_start);
            let iso_file_offset = extent
                .physical_lba
                .checked_mul(iso_block_size)
                .ok_or(Status::LOAD_ERROR)?;

            self.append_iso_file_range_to_source_segments(
                &mut segments,
                file_virtual_start,
                iso_file_offset,
                byte_count,
            )?;
        }

        if segments.is_empty() {
            Err(Status::DEVICE_ERROR.into())
        } else {
            Ok(segments)
        }
    }

    fn append_iso_file_range_to_source_segments(
        &self,
        segments: &mut Vec<WimbootMappedSegment>,
        virtual_start: u64,
        iso_file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        if byte_count == 0 {
            return Ok(());
        }

        let source_block_size = u64::from(self.iso.block_size);
        if source_block_size == 0 {
            return Err(Status::INVALID_PARAMETER.into());
        }

        if self.iso.extents.is_empty() {
            let physical_offset = self
                .iso
                .start_lba
                .checked_mul(source_block_size)
                .and_then(|start| start.checked_add(iso_file_offset))
                .ok_or(Status::LOAD_ERROR)?;
            segments
                .try_reserve_exact(1)
                .map_err(|_| Status::OUT_OF_RESOURCES)?;
            segments.push(WimbootMappedSegment {
                virtual_offset: virtual_start,
                physical_offset,
                byte_count,
            });
            return Ok(());
        }

        let file_end = iso_file_offset
            .checked_add(byte_count)
            .ok_or(Status::LOAD_ERROR)?;
        let mut cursor = iso_file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &self.iso.extents {
                let extent_file_start = extent
                    .virtual_block_start
                    .checked_mul(source_block_size)
                    .ok_or(Status::LOAD_ERROR)?;
                let extent_bytes = extent
                    .block_count
                    .checked_mul(source_block_size)
                    .ok_or(Status::LOAD_ERROR)?;
                let extent_file_end = extent_file_start
                    .checked_add(extent_bytes)
                    .ok_or(Status::LOAD_ERROR)?;

                if cursor < extent_file_start || cursor >= extent_file_end {
                    continue;
                }

                let overlap_end = file_end.min(extent_file_end);
                let overlap_len = overlap_end - cursor;
                let segment_virtual_start = virtual_start
                    .checked_add(cursor - iso_file_offset)
                    .ok_or(Status::LOAD_ERROR)?;
                let physical_offset = extent
                    .physical_lba
                    .checked_mul(source_block_size)
                    .and_then(|start| start.checked_add(cursor - extent_file_start))
                    .ok_or(Status::LOAD_ERROR)?;

                segments
                    .try_reserve_exact(1)
                    .map_err(|_| Status::OUT_OF_RESOURCES)?;
                segments.push(WimbootMappedSegment {
                    virtual_offset: segment_virtual_start,
                    physical_offset,
                    byte_count: overlap_len,
                });
                cursor = overlap_end;
                mapped = true;
                break;
            }

            if !mapped {
                return Err(Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }

    fn map_image_file_range_to_physical(
        &self,
        table: &mut ByteMappingTable,
        virtual_start: u64,
        file_offset: u64,
        byte_count: u64,
    ) -> uefi::Result<()> {
        let source_block_size = u64::from(self.iso.block_size);
        if source_block_size == 0 {
            return Err(uefi::Status::INVALID_PARAMETER.into());
        }

        if self.iso.extents.is_empty() {
            let physical_start = self
                .iso
                .start_lba
                .checked_mul(source_block_size)
                .and_then(|start| start.checked_add(file_offset))
                .ok_or(uefi::Status::LOAD_ERROR)?;
            table.add_segment(virtual_start, byte_count, physical_start);
            return Ok(());
        }

        let file_end = file_offset
            .checked_add(byte_count)
            .ok_or(uefi::Status::LOAD_ERROR)?;
        let mut cursor = file_offset;

        while cursor < file_end {
            let mut mapped = false;
            for extent in &self.iso.extents {
                let extent_file_start = extent
                    .virtual_block_start
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_bytes = extent
                    .block_count
                    .checked_mul(source_block_size)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let extent_file_end = extent_file_start
                    .checked_add(extent_bytes)
                    .ok_or(uefi::Status::LOAD_ERROR)?;

                if cursor < extent_file_start || cursor >= extent_file_end {
                    continue;
                }

                let overlap_end = file_end.min(extent_file_end);
                let overlap_len = overlap_end - cursor;
                let physical_start = extent
                    .physical_lba
                    .checked_mul(source_block_size)
                    .and_then(|start| start.checked_add(cursor - extent_file_start))
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                let segment_virtual_start = virtual_start
                    .checked_add(cursor - file_offset)
                    .ok_or(uefi::Status::LOAD_ERROR)?;
                table.add_segment(segment_virtual_start, overlap_len, physical_start);
                cursor = overlap_end;
                mapped = true;
                break;
            }

            if !mapped {
                return Err(uefi::Status::DEVICE_ERROR.into());
            }
        }

        Ok(())
    }

    fn open_virtual_iso_filesystem(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualIsoFilesystem> {
        let config = self.iso9660_virtual_config();
        if self.iso.is_udf {
            let vbio =
                self.build_source_backed_virtual_block_io(config.clone(), source_block_io)?;
            match Udf::open(Rc::new(VirtualIsoBlockIo::new(vbio))) {
                Ok(udf) => {
                    info!("Using UDF SimpleFileSystem backend for {}", self.iso.path);
                    return Ok(VirtualIsoFilesystem::Udf(udf));
                }
                Err(err) => {
                    warn!(
                        "Failed to open UDF filesystem for {}, falling back to ISO9660: {:?}",
                        self.iso.path, err
                    );
                }
            }
        }

        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        let iso = Iso9660::open(Rc::new(VirtualIsoBlockIo::new(vbio)))
            .map_err(fs_error_to_uefi_status)?;
        Ok(VirtualIsoFilesystem::Iso9660(iso))
    }

    fn install_iso_simple_file_system(
        &self,
        source_block_io: &BlockIO,
        virtual_handle: Handle,
        replacements: Vec<VirtualFileReplacement>,
    ) -> uefi::Result<RegisteredIsoSimpleFileSystem> {
        let fs = self.open_virtual_iso_filesystem(source_block_io)?;
        let block_size = fs.block_size();
        IsoSimpleFileSystemProtocol::install(
            self.bt,
            virtual_handle,
            Rc::new(fs),
            self.iso.size,
            block_size,
            replacements,
        )
    }

    fn preload_load_file_entries(&self) -> Vec<PreloadedFile> {
        let mut entries = Vec::new();
        if !self.iso.image_format.is_iso() {
            return entries;
        }

        for path in generic_efi_boot_paths() {
            match self.load_file(path) {
                Ok(data) if !data.is_empty() => {
                    let key = normalize_load_file_key(path);
                    if entries
                        .iter()
                        .any(|entry: &PreloadedFile| entry.path == key)
                    {
                        continue;
                    }

                    info!("Preloaded LoadFile path {} ({} bytes)", path, data.len());
                    entries.push(PreloadedFile { path: key, data });
                }
                Ok(_) => {}
                Err(err) => {
                    info!("LoadFile preload skipped {}: {:?}", path, err.status());
                }
            }
        }

        entries
    }

    fn boot_virtual_device(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Connecting virtual boot device {:?}", device.handle);
        if let Err(err) = self.bt.connect_controller(device.handle, None, None, true) {
            warn!(
                "ConnectController on virtual device returned {:?}; trying LoadImage anyway",
                err.status()
            );
        }

        if !self.iso.image_format.is_iso() {
            match self.boot_virtual_disk_partitions(device) {
                Ok(()) => return Ok(()),
                Err(err) => warn!(
                    "Virtual disk partition boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                ),
            }
        }

        self.try_load_image_paths(
            device.handle,
            &device.device_path,
            default_efi_boot_paths(),
            "virtual device",
        )
    }

    fn boot_virtual_disk_partitions(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;
        for attempt in 0..3 {
            let fs_handles = self
                .bt
                .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
            let mut matched_partitions = 0usize;

            for handle in fs_handles.iter().copied() {
                let Ok(partition_path) = self.handle_device_path_bytes(handle) else {
                    continue;
                };

                if !is_child_device_path(&device.device_path, &partition_path) {
                    continue;
                }

                matched_partitions += 1;
                info!(
                    "Found virtual disk filesystem partition {:?} for {}",
                    handle, self.iso.path
                );

                match self.try_load_image_paths(
                    handle,
                    &partition_path,
                    default_efi_boot_paths(),
                    "virtual disk partition",
                ) {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        last_status = err.status();
                        warn!(
                            "No bootable EFI path on virtual partition {:?}: {:?}",
                            handle,
                            err.status()
                        );
                    }
                }
            }

            if matched_partitions > 0 {
                return Err(last_status.into());
            }

            warn!(
                "No SimpleFileSystem partitions found under {} (attempt {}/3)",
                self.iso.path,
                attempt + 1
            );
            self.bt.stall(2_000_000);
        }

        Err(last_status.into())
    }

    fn handle_device_path_bytes(&self, handle: Handle) -> uefi::Result<Vec<u8>> {
        let device_path = unsafe {
            self.bt.open_protocol::<DevicePath>(
                OpenProtocolParams {
                    handle,
                    agent: self.parent_image,
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }?;

        device_path_to_vec(&device_path)
    }
}

struct VirtualBootDevice {
    handle: Handle,
    device_path: Vec<u8>,
}

struct LoadOptionsBuffer {
    data: Vec<u16>,
}

impl LoadOptionsBuffer {
    fn new(options: &str) -> uefi::Result<Self> {
        if options.bytes().any(|byte| byte == 0) {
            return Err(Status::INVALID_PARAMETER.into());
        }

        let mut data = Vec::new();
        data.try_reserve_exact(options.len().saturating_add(1))
            .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
        data.extend(options.encode_utf16());
        data.push(0);

        let _ = u32::try_from(
            data.len()
                .checked_mul(core::mem::size_of::<u16>())
                .ok_or(uefi::Status::OUT_OF_RESOURCES)?,
        )
        .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;

        Ok(Self { data })
    }

    fn size_bytes(&self) -> u32 {
        u32::try_from(self.data.len() * core::mem::size_of::<u16>()).unwrap_or(u32::MAX)
    }

    fn as_ptr(&self) -> *const c_void {
        self.data.as_ptr().cast::<c_void>()
    }
}

fn virtio_error_to_uefi_status(err: VirtIoError) -> Status {
    match err {
        VirtIoError::OutOfBounds | VirtIoError::InvalidMapping => Status::LOAD_ERROR,
        VirtIoError::WriteProtected => Status::WRITE_PROTECTED,
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => Status::INVALID_PARAMETER,
        VirtIoError::MediaChanged => Status::MEDIA_CHANGED,
        VirtIoError::NoPhysicalRead => Status::NO_MEDIA,
        VirtIoError::CrcError => Status::CRC_ERROR,
        VirtIoError::ReadFailed | VirtIoError::DeviceError => Status::DEVICE_ERROR,
    }
}

struct WimbootRuntimeInputs {
    runtime_files: Vec<WimbootRuntimeFile>,
    virtual_files: Vec<WimbootVirtualFile<'static>>,
}

#[derive(Default)]
struct WimbootInternalFiles {
    bootmgfw: Option<WimbootRuntimeFile>,
    bcd: Option<WimbootRuntimeFile>,
    boot_sdi: Option<WimbootRuntimeFile>,
    winpeshl: Option<Vec<u8>>,
}

struct WimbootWimImage {
    metadata: wim::WimMetadata,
    lookup: Vec<u8>,
    image_metadata: Vec<u8>,
}

struct WimbootRuntimeContext {
    reader: SourceVolumeReader,
    files: Vec<WimbootRuntimeFile>,
}

impl WimbootRuntimeContext {
    fn find_file(&self, path: &[u8]) -> Option<&WimbootRuntimeFile> {
        self.files
            .iter()
            .find(|file| file.callback_path.as_bytes() == path)
    }
}

#[derive(Clone)]
struct WimbootRuntimeFile {
    callback_path: String,
    size: u64,
    storage: WimbootRuntimeFileStorage,
}

#[derive(Clone)]
struct WimbootMappedSegment {
    virtual_offset: u64,
    physical_offset: u64,
    byte_count: u64,
}

#[derive(Clone)]
enum WimbootRuntimeFileStorage {
    Disk {
        block_size: u32,
        extents: Vec<IsoExtent>,
    },
    MappedBytes(Vec<WimbootMappedSegment>),
    Memory(Vec<u8>),
    WimResource {
        wim: Box<WimbootRuntimeFile>,
        metadata: wim::WimMetadata,
        resource: wim::WimResourceHeader,
    },
}

impl WimbootRuntimeFileStorage {
    fn read_range(&self, reader: &SourceVolumeReader, offset: u64, buf: &mut [u8]) -> Option<()> {
        match self {
            Self::Disk {
                block_size,
                extents,
            } => read_extent_range(reader, *block_size, extents, offset, buf),
            Self::MappedBytes(segments) => read_mapped_byte_range(reader, segments, offset, buf),
            Self::Memory(data) => {
                let start = usize::try_from(offset).ok()?;
                let end = start.checked_add(buf.len())?;
                buf.copy_from_slice(data.get(start..end)?);
                Some(())
            }
            Self::WimResource {
                wim,
                metadata,
                resource,
            } => wim::read_resource_range_with(
                metadata,
                wim.size,
                resource,
                offset,
                buf,
                |wim_offset, wim_buf| {
                    wim.read_range(reader, wim_offset, wim_buf)
                        .ok_or(wim::WimReadError::ResourceOutOfBounds)
                },
            )
            .ok(),
        }
    }
}

impl WimbootRuntimeFile {
    fn from_iso(iso: &IsoFile, callback_path: &str) -> uefi::Result<Self> {
        if iso.extents.is_empty() || iso.block_size == 0 {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size: iso.size,
            storage: WimbootRuntimeFileStorage::Disk {
                block_size: iso.block_size,
                extents: iso.extents.clone(),
            },
        })
    }

    fn from_source_file(
        file: &SourceVolumeFileMetadata,
        callback_path: &str,
    ) -> uefi::Result<Self> {
        if file.extents.is_empty() || file.block_size == 0 {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size: file.size,
            storage: WimbootRuntimeFileStorage::Disk {
                block_size: file.block_size,
                extents: file.extents.clone(),
            },
        })
    }

    fn from_mapped_segments(
        callback_path: &str,
        size: u64,
        segments: Vec<WimbootMappedSegment>,
    ) -> uefi::Result<Self> {
        if size != 0 && segments.is_empty() {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size,
            storage: WimbootRuntimeFileStorage::MappedBytes(segments),
        })
    }

    fn from_memory(callback_path: &str, data: Vec<u8>) -> Self {
        Self {
            callback_path: String::from(callback_path),
            size: data.len() as u64,
            storage: WimbootRuntimeFileStorage::Memory(data),
        }
    }

    fn from_wim_resource(
        callback_path: &str,
        wim: &WimbootRuntimeFile,
        metadata: wim::WimMetadata,
        resource: wim::WimResourceHeader,
    ) -> Self {
        Self {
            callback_path: String::from(callback_path),
            size: resource.uncompressed_size,
            storage: WimbootRuntimeFileStorage::WimResource {
                wim: Box::new(wim.clone()),
                metadata,
                resource,
            },
        }
    }

    fn size_i32(&self) -> Option<i32> {
        i32::try_from(self.size).ok()
    }

    fn read_range(&self, reader: &SourceVolumeReader, offset: u64, buf: &mut [u8]) -> Option<()> {
        let end = offset.checked_add(buf.len() as u64)?;
        if end > self.size {
            return None;
        }

        self.storage.read_range(reader, offset, buf)
    }
}

fn read_extent_range(
    reader: &SourceVolumeReader,
    block_size: u32,
    extents: &[IsoExtent],
    offset: u64,
    buf: &mut [u8],
) -> Option<()> {
    let end = offset.checked_add(buf.len() as u64)?;
    let block_size_u64 = u64::from(block_size);
    let mut cursor = offset;
    let mut copied = 0usize;

    while copied < buf.len() {
        let extent = extents.iter().find(|extent| {
            let Some(extent_start) = extent.virtual_block_start.checked_mul(block_size_u64) else {
                return false;
            };
            let Some(extent_bytes) = extent.block_count.checked_mul(block_size_u64) else {
                return false;
            };
            let Some(extent_end) = extent_start.checked_add(extent_bytes) else {
                return false;
            };
            cursor >= extent_start && cursor < extent_end
        })?;

        let extent_start = extent.virtual_block_start.checked_mul(block_size_u64)?;
        let extent_bytes = extent.block_count.checked_mul(block_size_u64)?;
        let extent_end = extent_start.checked_add(extent_bytes)?;
        let read_end = end.min(extent_end);
        let read_len = usize::try_from(read_end.checked_sub(cursor)?).ok()?;
        let physical_byte = extent
            .physical_lba
            .checked_mul(block_size_u64)?
            .checked_add(cursor.checked_sub(extent_start)?)?;

        read_physical_bytes(
            reader,
            block_size,
            physical_byte,
            &mut buf[copied..copied + read_len],
        )?;

        cursor = read_end;
        copied += read_len;
    }

    Some(())
}

fn read_mapped_byte_range(
    reader: &SourceVolumeReader,
    segments: &[WimbootMappedSegment],
    offset: u64,
    buf: &mut [u8],
) -> Option<()> {
    let end = offset.checked_add(buf.len() as u64)?;
    let mut cursor = offset;
    let mut copied = 0usize;

    while copied < buf.len() {
        let segment = segments.iter().find(|segment| {
            let Some(segment_end) = segment.virtual_offset.checked_add(segment.byte_count) else {
                return false;
            };
            cursor >= segment.virtual_offset && cursor < segment_end
        })?;

        let segment_end = segment.virtual_offset.checked_add(segment.byte_count)?;
        let read_end = end.min(segment_end);
        let read_len = usize::try_from(read_end.checked_sub(cursor)?).ok()?;
        let physical_byte = segment
            .physical_offset
            .checked_add(cursor.checked_sub(segment.virtual_offset)?)?;

        read_physical_bytes(
            reader,
            reader.block_size(),
            physical_byte,
            &mut buf[copied..copied + read_len],
        )?;

        cursor = read_end;
        copied += read_len;
    }

    Some(())
}

struct WimbootRuntimeRegistration<'a> {
    context: *mut WimbootRuntimeContext,
    previous: *mut WimbootRuntimeContext,
    _source_block_io: ScopedProtocol<'a, BlockIO>,
}

impl<'a> WimbootRuntimeRegistration<'a> {
    fn install(
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

    fn callbacks(&self) -> WimbootCallbacks {
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

fn read_physical_bytes(
    reader: &SourceVolumeReader,
    block_size: u32,
    physical_byte_start: u64,
    buf: &mut [u8],
) -> Option<()> {
    let block_size = usize::try_from(block_size).ok()?;
    if block_size == 0 {
        return None;
    }

    let mut scratch = Vec::new();
    scratch.try_reserve_exact(block_size).ok()?;
    scratch.resize(block_size, 0);

    let mut physical_byte = physical_byte_start;
    let mut copied = 0usize;

    while copied < buf.len() {
        let physical_lba = physical_byte / block_size as u64;
        let in_block_offset = usize::try_from(physical_byte % block_size as u64).ok()?;
        let copy_size = (block_size - in_block_offset).min(buf.len() - copied);

        PhysicalReader::read_blocks(reader, physical_lba, &mut scratch).ok()?;
        buf[copied..copied + copy_size]
            .copy_from_slice(&scratch[in_block_offset..in_block_offset + copy_size]);

        physical_byte = physical_byte.checked_add(copy_size as u64)?;
        copied += copy_size;
    }

    Some(())
}

struct VirtualIsoBlockIo {
    vbio: VirtualBlockIo,
    media_id: u32,
}

impl VirtualIsoBlockIo {
    fn new(vbio: VirtualBlockIo) -> Self {
        let media_id = vbio.media_id();
        Self { vbio, media_id }
    }
}

impl BlockIoOps for VirtualIsoBlockIo {
    fn block_size(&self) -> u32 {
        self.vbio.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.vbio.block_count()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.vbio
            .read_blocks(self.media_id, lba, buf)
            .map_err(virtio_error_to_fs_error)
    }
}

fn virtio_error_to_fs_error(err: VirtIoError) -> FsError {
    match err {
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => FsError::InvalidArgument,
        VirtIoError::OutOfBounds
        | VirtIoError::MediaChanged
        | VirtIoError::InvalidMapping
        | VirtIoError::NoPhysicalRead
        | VirtIoError::ReadFailed
        | VirtIoError::DeviceError
        | VirtIoError::CrcError => FsError::ReadError,
        VirtIoError::WriteProtected => FsError::UnsupportedFs,
    }
}

fn fs_error_to_uefi_status(err: FsError) -> Status {
    match err {
        FsError::FileNotFound | FsError::DirectoryNotFound => Status::NOT_FOUND,
        FsError::InvalidPath | FsError::InvalidArgument => Status::INVALID_PARAMETER,
        FsError::OutOfMemory | FsError::FileTooLarge => Status::OUT_OF_RESOURCES,
        FsError::NotDirectory | FsError::NotFile | FsError::UnsupportedFs => Status::UNSUPPORTED,
        FsError::InvalidSignature | FsError::BlockSizeMismatch | FsError::Corrupted => {
            Status::LOAD_ERROR
        }
        FsError::ReadError => Status::DEVICE_ERROR,
    }
}

fn xz_error_to_uefi_status(err: XzDecodeError) -> Status {
    match err {
        XzDecodeError::OutputTooLarge | XzDecodeError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
        XzDecodeError::Decoder(_) | XzDecodeError::Stalled => Status::LOAD_ERROR,
    }
}

fn wim_read_error_to_uefi_status(err: wim::WimReadError) -> Status {
    match err {
        wim::WimReadError::InvalidChunkLength
        | wim::WimReadError::InvalidRange
        | wim::WimReadError::InvalidChunkTable
        | wim::WimReadError::XpressDecodeFailed(_)
        | wim::WimReadError::LzxDecodeFailed(_) => Status::LOAD_ERROR,
        wim::WimReadError::ResourceOutOfBounds => Status::DEVICE_ERROR,
        wim::WimReadError::OutputReserveFailed => Status::OUT_OF_RESOURCES,
        wim::WimReadError::UnsupportedCompressedChunk { .. } => Status::UNSUPPORTED,
    }
}

fn ventoy_error_to_uefi_status(err: crate::ventoy::VentoyParamError) -> Status {
    match err {
        crate::ventoy::VentoyParamError::PathTooLong
        | crate::ventoy::VentoyParamError::InvalidSectorSize => Status::INVALID_PARAMETER,
        crate::ventoy::VentoyParamError::UnalignedExtent => Status::UNSUPPORTED,
        crate::ventoy::VentoyParamError::ValueOutOfRange
        | crate::ventoy::VentoyParamError::OutputTooLarge
        | crate::ventoy::VentoyParamError::OutputReserveFailed => Status::OUT_OF_RESOURCES,
    }
}

fn ventoy_linux_error_to_uefi_status(err: crate::ventoy_linux::VentoyLinuxInitrdError) -> Status {
    match err {
        crate::ventoy_linux::VentoyLinuxInitrdError::InvalidArchive => Status::LOAD_ERROR,
        crate::ventoy_linux::VentoyLinuxInitrdError::InvalidSectorSize
        | crate::ventoy_linux::VentoyLinuxInitrdError::NameTooLong => Status::INVALID_PARAMETER,
        crate::ventoy_linux::VentoyLinuxInitrdError::UnalignedExtent => Status::UNSUPPORTED,
        crate::ventoy_linux::VentoyLinuxInitrdError::ValueOutOfRange
        | crate::ventoy_linux::VentoyLinuxInitrdError::FileTooLarge
        | crate::ventoy_linux::VentoyLinuxInitrdError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}

fn ventoy_windows_runtime_data_error_to_uefi_status(
    err: nextboot_windows::VentoyWindowsRuntimeDataError,
) -> Status {
    match err {
        nextboot_windows::VentoyWindowsRuntimeDataError::AutoInstallTooLarge
        | nextboot_windows::VentoyWindowsRuntimeDataError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}

fn ventoy_windows_wimboot_payload_error_to_uefi_status(
    err: nextboot_windows::VentoyWindowsWimbootPayloadError,
) -> Status {
    match err {
        nextboot_windows::VentoyWindowsWimbootPayloadError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}

fn selected_ventoy_plugin_index(autosel: Option<usize>, count: usize) -> Option<usize> {
    match autosel {
        Some(0) | None => None,
        Some(value) if value <= count => Some(value - 1),
        _ => None,
    }
}

fn selected_persistence_backend_index(autosel: Option<usize>, count: usize) -> Option<usize> {
    match autosel {
        Some(0) => None,
        Some(value) => selected_ventoy_plugin_index(Some(value), count),
        None if count == 1 => Some(0),
        None => None,
    }
}

fn iso9660_file_extent_patch(first_sector: u32, size: u32) -> Vec<u8> {
    let mut patch = Vec::new();
    patch.extend_from_slice(&first_sector.to_le_bytes());
    patch.extend_from_slice(&first_sector.to_be_bytes());
    patch.extend_from_slice(&size.to_le_bytes());
    patch.extend_from_slice(&size.to_be_bytes());
    patch
}

fn normalize_iso_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = trimmed.trim_start_matches(['/', '\\']);
    let mut normalized = String::from("/");
    let mut first = true;

    for part in trimmed
        .split(|ch| ch == '/' || ch == '\\')
        .filter(|part| !part.is_empty())
    {
        if !first {
            normalized.push('/');
        }
        normalized.push_str(part);
        first = false;
    }

    normalized
}

fn push_unique_iso_path(paths: &mut Vec<String>, path: &str) -> uefi::Result<()> {
    let normalized = normalize_iso_path(path);
    if paths.iter().any(|existing| existing == &normalized) {
        return Ok(());
    }

    paths
        .try_reserve_exact(1)
        .map_err(|_| uefi::Status::OUT_OF_RESOURCES)?;
    paths.push(normalized);
    Ok(())
}

fn is_isolinux_config_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/isolinux/") || lower.contains("/syslinux/") || lower.ends_with("isolinux.cfg")
}

fn iso_parent_dir(path: &str) -> String {
    let normalized = normalize_iso_path(path);
    match normalized.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(index) => String::from(&normalized[..index]),
    }
}

fn resolve_linux_config_path(base_dir: &str, path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return normalize_iso_path(trimmed);
    }

    if base_dir == "/" {
        normalize_iso_path(trimmed)
    } else {
        normalize_iso_path(&format!("{}/{}", base_dir, trimmed))
    }
}

fn runtime_extent_count(iso: &IsoFile) -> usize {
    if iso.extents.is_empty() {
        1
    } else {
        iso.extents.len()
    }
}

fn device_path_to_vec(device_path: &DevicePath) -> uefi::Result<Vec<u8>> {
    let ptr = device_path.as_ffi_ptr().cast::<u8>();
    let len = unsafe { device_path_byte_len(ptr) }.ok_or(uefi::Status::INVALID_PARAMETER)?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    Ok(bytes.to_vec())
}

unsafe fn device_path_byte_len(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }

    let mut offset = 0usize;
    loop {
        let node = ptr.add(offset);
        let node_type = ptr::read_unaligned(node);
        let node_subtype = ptr::read_unaligned(node.add(1));
        let len_lo = ptr::read_unaligned(node.add(2));
        let len_hi = ptr::read_unaligned(node.add(3));
        let node_len = u16::from_le_bytes([len_lo, len_hi]) as usize;
        if node_len < 4 {
            return None;
        }

        offset = offset.checked_add(node_len)?;
        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }
    }
}

fn is_child_device_path(parent: &[u8], child: &[u8]) -> bool {
    let parent_prefix_len = parent_without_end_len(parent).unwrap_or(parent.len());
    child.len() >= parent_prefix_len
        && child.get(..parent_prefix_len) == parent.get(..parent_prefix_len)
}

fn parent_without_end_len(path: &[u8]) -> Option<usize> {
    if path.len() < 4 {
        return None;
    }

    let mut offset = 0usize;
    while offset.checked_add(4)? <= path.len() {
        let node_type = *path.get(offset)?;
        let node_subtype = *path.get(offset + 1)?;
        let node_len =
            u16::from_le_bytes([*path.get(offset + 2)?, *path.get(offset + 3)?]) as usize;
        if node_len < 4 || offset.checked_add(node_len)? > path.len() {
            return None;
        }

        if node_type == 0x7F && node_subtype == 0xFF {
            return Some(offset);
        }

        offset += node_len;
    }

    None
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn div_round_up(value: u64, divisor: u64) -> Option<u64> {
    if divisor == 0 {
        return None;
    }

    value.checked_add(divisor - 1).map(|value| value / divisor)
}

fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    if align == 0 {
        return None;
    }

    let remainder = value % align;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(align - remainder)
    }
}

fn usize_to_u16(value: usize) -> uefi::Result<u16> {
    u16::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

fn usize_to_u32(value: usize) -> uefi::Result<u32> {
    u32::try_from(value).map_err(|_| uefi::Status::OUT_OF_RESOURCES.into())
}

fn os_type_code(os_type: OsType) -> u32 {
    match os_type {
        OsType::Unknown => 0,
        OsType::Windows => 1,
        OsType::WinPE => 2,
        OsType::Linux => 10,
        OsType::Ubuntu => 11,
        OsType::Debian => 12,
        OsType::Fedora => 13,
        OsType::Arch => 14,
    }
}

fn ventoy_chain_type(iso: &IsoFile) -> u8 {
    if iso.image_format.is_wim_container() {
        return crate::ventoy::VENTOY_CHAIN_WIM;
    }

    match iso.os_type {
        OsType::Windows | OsType::WinPE => crate::ventoy::VENTOY_CHAIN_WINDOWS,
        _ => crate::ventoy::VENTOY_CHAIN_LINUX,
    }
}

fn virtual_device_type_code(device_type: VirtualDeviceType) -> u32 {
    match device_type {
        VirtualDeviceType::DvdRom => 1,
        VirtualDeviceType::HardDisk => 2,
        VirtualDeviceType::UsbMassStorage => 3,
    }
}

fn push_u16(data: &mut Vec<u8>, value: u16) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(data: &mut Vec<u8>, value: u64) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn push_extent_record(
    data: &mut Vec<u8>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    push_u64(data, virtual_block_start);
    push_u64(data, physical_lba);
    push_u64(data, block_count);
}

/// 引导模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// 直接内核启动
    DirectKernel,
    /// EFI 链式加载
    ChainLoad,
    /// 虚拟设备引导
    VirtualDevice,
    /// 内存引导
    MemDisk,
}

impl Default for BootMode {
    fn default() -> Self {
        BootMode::VirtualDevice
    }
}

/// 引导选项
#[derive(Debug, Clone)]
pub struct BootOptions {
    /// 引导模式
    pub mode: BootMode,
    /// 内核参数
    pub kernel_args: String,
    /// 是否启用调试
    pub debug: bool,
    /// 超时 (秒)
    pub timeout: Option<u64>,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            mode: BootMode::default(),
            kernel_args: String::new(),
            debug: false,
            timeout: None,
        }
    }
}

/// 内存映射信息
#[derive(Debug, Clone)]
pub struct MemoryMapInfo {
    /// 起始地址
    pub start: u64,
    /// 大小
    pub size: u64,
    /// 类型
    pub memory_type: u32,
}

/// 分配引导内存
pub fn allocate_boot_memory(bt: &BootServices, size: usize) -> uefi::Result<*mut u8> {
    use uefi::table::boot::MemoryType;

    let pages = (size + 4095) / 4096;

    bt.allocate_pages(
        uefi::table::boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        pages,
    )
    .map(|addr| addr as *mut u8)
}

/// 释放引导内存
pub fn free_boot_memory(bt: &BootServices, ptr: *mut u8, size: usize) -> uefi::Result<()> {
    let pages = (size + 4095) / 4096;
    unsafe { bt.free_pages(ptr as u64, pages) }
}
