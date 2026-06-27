//! ISO9660 文件系统实现
//!
//! 用于解析 ISO 镜像内部结构

mod detect;
mod directory;
mod eltorito;
mod fs;

pub use detect::{detect_os_type, IsoOsType};
pub use eltorito::{
    detect_udf_volume, get_eltorito_boot_info, is_bootable_iso, read_efi_eltorito_boot_info,
    read_eltorito_boot_info, ElToritoBootInfo,
};
pub use fs::{Iso9660, IsoDirectoryRecordLocation};

pub(crate) const ISO_SECTOR_SIZE: usize = 2048;
pub(crate) const EL_TORITO_BOOT_RECORD_LBA: u64 = 17;
pub(crate) const EL_TORITO_PLATFORM_EFI: u8 = 0xEF;
pub(crate) const EL_TORITO_BOOTABLE: u8 = 0x88;
pub(crate) const EL_TORITO_SECTION_HEADER: u8 = 0x90;
pub(crate) const EL_TORITO_FINAL_SECTION_HEADER: u8 = 0x91;
pub(crate) const UDF_PROBE_START_LBA: u64 = 16;
pub(crate) const UDF_PROBE_END_LBA: u64 = 32;
