//! Windows 引导支持
//!
//! 实现 Windows ISO 的启动，这是项目中最复杂的部分。
//!
//! # 挑战
//! Windows 启动后会重新初始化 USB 驱动，导致虚拟设备丢失。
//! 需要特殊的内存补丁来解决此问题。
//!
//! # 需求对应
//! - 模块 C: Windows 引导 (P1)
//! - 模块 B: Block IO 劫持 (P0)

#![no_std]

extern crate alloc;

pub mod acpi;
pub mod bcd;

mod boot;
mod pe;
mod ventoy;

pub use boot::{WindowsBootConfig, WindowsBootError, WindowsBootMethod, WindowsBootloader};
pub use pe::{is_windows_iso, PeInfo};
pub use ventoy::{
    build_ventoy_wimboot_jump_payload, build_ventoy_windows_runtime_data, VentoyWindowsAutoInstall,
    VentoyWindowsRuntimeDataError, VentoyWindowsRuntimeDataInput, VentoyWindowsWimbootPayloadError,
    VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE, VENTOY_WINDOWS_DATA_HEADER_SIZE,
    VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE, VENTOY_WINDOWS_DATA_RESERVED_SIZE,
};

pub use boot::WindowsVersion;
