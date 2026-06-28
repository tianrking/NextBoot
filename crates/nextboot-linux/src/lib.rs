//! Linux 引导支持
//!
//! 负责从 ISO 中提取 Linux Kernel 和 Initrd 并启动
//!
//! # 支持的发行版
//! - Ubuntu (casper)
//! - Debian (install.amd)
//! - Fedora (images/pxeboot)
//! - Arch (arch/boot)
//! - 通用 Linux (grub/isolinux)
//!
//! # 需求对应
//! - 模块 C: Linux 引导 (P0)

#![no_std]

extern crate alloc;

mod config;
mod distro;
mod loader;
mod params;
mod parser;

pub use config::{auto_detect_config, LinuxBootConfig};
pub use distro::LinuxDistro;
pub use loader::{LinuxBootError, LinuxBootloader};
pub use params::{BootParams, EfiHandoverParams, EfiStubOptions};
pub use parser::{
    parse_grub_boot_entry, parse_grub_cfg, parse_isolinux_boot_entry, parse_isolinux_cfg,
    LinuxBootEntry,
};
