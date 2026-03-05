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
//! # PRD 对应
//! - 模块 C: Linux 引导 (P0)

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::boxed::Box;
use log::{info, warn, error};

/// Linux 发行版类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDistro {
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    OpenSuse,
    CentOS,
    LinuxMint,
    PopOs,
    Manjaro,
    Generic,
}

impl LinuxDistro {
    /// 从 ISO 文件列表检测发行版
    pub fn detect(files: &[&str]) -> Self {
        for file in files {
            let f = file.to_lowercase();

            // Ubuntu 及衍生版
            if f.contains("casper/vmlinuz") {
                if f.contains("linuxmint") {
                    return LinuxDistro::LinuxMint;
                }
                if f.contains("pop-os") || f.contains("popos") {
                    return LinuxDistro::PopOs;
                }
                return LinuxDistro::Ubuntu;
            }

            // Debian
            if f.contains("install.amd") || f.contains("install.386") {
                return LinuxDistro::Debian;
            }

            // Fedora
            if f.contains("images/pxeboot") || f.contains("fedora") {
                return LinuxDistro::Fedora;
            }

            // Arch 及衍生版
            if f.contains("arch/boot") {
                if f.contains("manjaro") {
                    return LinuxDistro::Manjaro;
                }
                return LinuxDistro::Arch;
            }

            // openSUSE
            if f.contains("boot/x86_64/loader") || f.contains("opensuse") {
                return LinuxDistro::OpenSuse;
            }

            // CentOS / RHEL
            if f.contains("images/pxeboot") && f.contains("centos") {
                return LinuxDistro::CentOS;
            }
        }

        LinuxDistro::Generic
    }

    /// 获取显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            LinuxDistro::Ubuntu => "Ubuntu",
            LinuxDistro::Debian => "Debian",
            LinuxDistro::Fedora => "Fedora",
            LinuxDistro::Arch => "Arch Linux",
            LinuxDistro::OpenSuse => "openSUSE",
            LinuxDistro::CentOS => "CentOS",
            LinuxDistro::LinuxMint => "Linux Mint",
            LinuxDistro::PopOs => "Pop!_OS",
            LinuxDistro::Manjaro => "Manjaro",
            LinuxDistro::Generic => "Linux",
        }
    }
}

/// Linux 启动配置
#[derive(Debug, Clone)]
pub struct LinuxBootConfig {
    /// 发行版类型
    pub distro: LinuxDistro,
    /// Kernel 文件路径
    pub kernel_path: String,
    /// Initrd 文件路径
    pub initrd_path: String,
    /// 内核命令行参数
    pub cmdline: String,
    /// ISO 文件路径 (用于 iso-scan)
    pub iso_path: String,
    /// 是否使用 UEFI 启动
    pub use_efi: bool,
}

impl LinuxBootConfig {
    /// 为指定发行版创建默认配置
    pub fn for_distro(distro: LinuxDistro, iso_path: &str) -> Self {
        let (kernel, initrd, extra_cmdline) = match distro {
            LinuxDistro::Ubuntu => (
                "/casper/vmlinuz",
                "/casper/initrd",
                "boot=casper quiet splash maybe-ubiquity"
            ),
            LinuxDistro::LinuxMint => (
                "/casper/vmlinuz",
                "/casper/initrd",
                "boot=casper quiet splash"
            ),
            LinuxDistro::PopOs => (
                "/casper/vmlinuz",
                "/casper/initrd",
                "boot=casper quiet splash"
            ),
            LinuxDistro::Debian => (
                "/install.amd/vmlinuz",
                "/install.amd/initrd.gz",
                "vga=788 -- quiet"
            ),
            LinuxDistro::Fedora => (
                "/images/pxeboot/vmlinuz",
                "/images/pxeboot/initrd.img",
                "root=live:CDLABEL=Fedora quiet rhgb"
            ),
            LinuxDistro::Arch => (
                "/arch/boot/x86_64/vmlinuz-linux",
                "/arch/boot/x86_64/initramfs-linux.img",
                "archisobasedir=arch archisolabel=ARCH_$(date +%Y%m)"
            ),
            LinuxDistro::Manjaro => (
                "/boot/vmlinuz-x86_64",
                "/boot/initramfs-x86_64.img",
                "driver=free tz=utc lang=en_US keytable=us"
            ),
            LinuxDistro::OpenSuse => (
                "/boot/x86_64/loader/linux",
                "/boot/x86_64/loader/initrd",
                "install=cd:/ quiet"
            ),
            LinuxDistro::CentOS => (
                "/images/pxeboot/vmlinuz",
                "/images/pxeboot/initrd.img",
                "inst.stage2=hd:LABEL=CentOS quiet"
            ),
            LinuxDistro::Generic => (
                "/boot/vmlinuz",
                "/boot/initrd.img",
                ""
            ),
        };

        // 构建完整的命令行
        let cmdline = match distro {
            LinuxDistro::Ubuntu | LinuxDistro::LinuxMint | LinuxDistro::PopOs => {
                format!(
                    "{} iso-scan/filename={} --",
                    extra_cmdline, iso_path
                )
            }
            LinuxDistro::Debian => {
                format!(
                    "{} findiso={}",
                    extra_cmdline, iso_path
                )
            }
            LinuxDistro::Arch => {
                // Arch 需要特殊处理
                format!(
                    "{} img_dev=/dev/disk/by-uuid/{{UUID}} img_loop={}",
                    extra_cmdline, iso_path
                )
            }
            _ => extra_cmdline.to_string(),
        };

        Self {
            distro,
            kernel_path: kernel.to_string(),
            initrd_path: initrd.to_string(),
            cmdline,
            iso_path: iso_path.to_string(),
            use_efi: true,
        }
    }

    /// 添加内核参数
    pub fn add_cmdline(&mut self, param: &str) {
        if !self.cmdline.is_empty() {
            self.cmdline.push(' ');
        }
        self.cmdline.push_str(param);
    }

    /// 设置内核参数
    pub fn set_cmdline(&mut self, cmdline: &str) {
        self.cmdline = cmdline.to_string();
    }
}

/// Linux 启动器
pub struct LinuxBootloader {
    config: LinuxBootConfig,
    kernel_data: Vec<u8>,
    initrd_data: Vec<u8>,
}

impl LinuxBootloader {
    /// 创建新的启动器
    pub fn new(config: LinuxBootConfig) -> Self {
        Self {
            config,
            kernel_data: Vec::new(),
            initrd_data: Vec::new(),
        }
    }

    /// 加载 Kernel
    pub fn load_kernel(&mut self, data: Vec<u8>) -> Result<(), LinuxBootError> {
        // 验证 Kernel 魔数
        if data.len() < 0x208 {
            return Err(LinuxBootError::InvalidKernel);
        }

        // Linux x86_64 内核魔数: "HdrS" 在偏移 0x202
        if &data[0x202..0x206] != b"HdrS" {
            return Err(LinuxBootError::InvalidKernel);
        }

        // 检查版本 (需要 2.08+)
        let version = u16::from_le_bytes([data[0x206], data[0x207]]);
        if version < 0x0208 {
            warn!("Linux kernel version too old: {}", version);
        }

        info!("Loaded Linux kernel: {} bytes", data.len());
        self.kernel_data = data;
        Ok(())
    }

    /// 加载 Initrd
    pub fn load_initrd(&mut self, data: Vec<u8>) -> Result<(), LinuxBootError> {
        // Initrd 可以是 gzip 或 cpio 格式
        if data.is_empty() {
            return Err(LinuxBootError::InvalidInitrd);
        }

        // 检查常见格式
        let is_gzip = data.len() >= 2 && data[0] == 0x1F && data[1] == 0x8B;
        let is_cpio_newc = data.len() >= 6 && &data[0..6] == b"070701";
        let is_cpio_odc = data.len() >= 6 && &data[0..6] == b"070707";
        let is_xz = data.len() >= 6 && data[0] == 0xFD && &data[1..6] == b"7zXZ\x00";
        let is_zstd = data.len() >= 4 && &data[0..4] == b"\x28\xb5\x2f\xfd";

        if !is_gzip && !is_cpio_newc && !is_cpio_odc && !is_xz && !is_zstd {
            warn!("Unknown initrd format, first bytes: {:02X?}", &data[0..8.min(data.len())]);
        }

        info!("Loaded initrd: {} bytes", data.len());
        self.initrd_data = data;
        Ok(())
    }

    /// 执行启动
    ///
    /// # 安全性
    /// 此函数不会返回，直接跳转到 Kernel
    pub unsafe fn boot(self) -> ! {
        info!("Booting Linux kernel...");

        // TODO: 实现 Linux Kernel 启动协议
        // 对于 UEFI 启动，有两种方式:
        // 1. EFI Handover Protocol (较新的内核支持)
        // 2. LoadImage/StartImage (通过 EFI stub)

        // 使用 EFI Handover Protocol:
        // 1. 找到内核中的 handover 入口点
        // 2. 设置 boot_params 结构
        // 3. 设置 initrd 地址和大小
        // 4. 设置命令行
        // 5. 跳转到 handover 入口点

        // 使用 EFI stub:
        // 1. 将内核作为 EFI 镜像加载
        // 2. 设置 initrd 和命令行
        // 3. 调用 StartImage

        loop {
            core::hint::spin_loop();
        }
    }

    /// 获取命令行
    pub fn cmdline(&self) -> &str {
        &self.config.cmdline
    }

    /// 获取 Kernel 大小
    pub fn kernel_size(&self) -> usize {
        self.kernel_data.len()
    }

    /// 获取 Initrd 大小
    pub fn initrd_size(&self) -> usize {
        self.initrd_data.len()
    }

    /// 检查是否准备好启动
    pub fn is_ready(&self) -> bool {
        !self.kernel_data.is_empty() && !self.initrd_data.is_empty()
    }

    /// 获取配置
    pub fn config(&self) -> &LinuxBootConfig {
        &self.config
    }
}

/// Linux 启动错误
#[derive(Debug, Clone, Copy)]
pub enum LinuxBootError {
    /// 无效的 Kernel
    InvalidKernel,
    /// 无效的 Initrd
    InvalidInitrd,
    /// 内存不足
    OutOfMemory,
    /// 加载失败
    LoadFailed,
    /// 配置文件解析失败
    ConfigParseError,
    /// 不支持的发行版
    UnsupportedDistro,
    /// UEFI 服务不可用
    UefiNotAvailable,
}

impl core::fmt::Display for LinuxBootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LinuxBootError::InvalidKernel => write!(f, "Invalid Linux kernel"),
            LinuxBootError::InvalidInitrd => write!(f, "Invalid initrd"),
            LinuxBootError::OutOfMemory => write!(f, "Out of memory"),
            LinuxBootError::LoadFailed => write!(f, "Failed to load kernel"),
            LinuxBootError::ConfigParseError => write!(f, "Failed to parse config"),
            LinuxBootError::UnsupportedDistro => write!(f, "Unsupported distribution"),
            LinuxBootError::UefiNotAvailable => write!(f, "UEFI services not available"),
        }
    }
}

/// 解析 isolinux/syslinux 配置
pub fn parse_isolinux_cfg(cfg: &str) -> Option<(String, String, String)> {
    let mut kernel = None;
    let mut initrd = None;
    let mut append = String::new();

    for line in cfg.lines() {
        let line = line.trim();
        let line_lower = line.to_lowercase();

        if line_lower.starts_with("kernel") || line_lower.starts_with("linux") {
            kernel = line.split_whitespace().nth(1).map(|s| s.to_string());
        } else if line_lower.starts_with("initrd") {
            initrd = line.split_whitespace().nth(1).map(|s| s.to_string());
        } else if line_lower.starts_with("append") {
            append = line.splitn(2, ' ').nth(1).unwrap_or("").to_string();
        }
    }

    match (kernel, initrd) {
        (Some(k), Some(i)) => Some((k, i, append)),
        _ => None,
    }
}

/// 解析 GRUB 配置
pub fn parse_grub_cfg(cfg: &str) -> Option<(String, String, String)> {
    let mut kernel = None;
    let mut initrd = None;
    let mut cmdline = String::new();

    for line in cfg.lines() {
        let line = line.trim();

        if line.starts_with("linux") || line.starts_with("linux16") || line.starts_with("linuxefi") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                kernel = Some(parts[1].to_string());
                if parts.len() > 2 {
                    cmdline = parts[2..].join(" ");
                }
            }
        } else if line.starts_with("initrd") || line.starts_with("initrd16") || line.starts_with("initrdefi") {
            initrd = line.split_whitespace().nth(1).map(|s| s.to_string());
        }
    }

    match (kernel, initrd) {
        (Some(k), Some(i)) => Some((k, i, cmdline)),
        _ => None,
    }
}

/// 从 ISO 文件列表自动检测启动配置
pub fn auto_detect_config(files: &[&str], iso_path: &str) -> Option<LinuxBootConfig> {
    let distro = LinuxDistro::detect(files);
    Some(LinuxBootConfig::for_distro(distro, iso_path))
}

/// Linux 内核启动参数结构
#[repr(C, packed)]
pub struct BootParams {
    // 屏幕信息
    pub orig_x: u8,
    pub orig_y: u8,
    pub ext_mem_k: u16,
    pub orig_video_page: u16,
    pub orig_video_mode: u8,
    pub orig_video_cols: u8,
    pub unused1: u16,
    pub orig_video_ega_bx: u16,
    pub unused2: u16,
    pub orig_video_lines: u8,
    pub orig_video_is_vga: u8,
    pub orig_video_points: u16,

    // VESA 信息
    pub lfb_width: u16,
    pub lfb_height: u16,
    pub lfb_depth: u16,
    pub lfb_base: u32,
    pub lfb_size: u32,
    pub cl_magic: u16,
    pub cl_offset: u16,
    pub lfb_linelength: u16,
    pub red_size: u8,
    pub red_pos: u8,
    pub green_size: u8,
    pub green_pos: u8,
    pub blue_size: u8,
    pub blue_pos: u8,
    pub rsvd_size: u8,
    pub rsvd_pos: u8,
    pub vesapm_seg: u16,
    pub vesapm_off: u16,
    pub pages: u16,
    pub vesa_attributes: u16,
    pub capabilities: u32,
    pub ext_lfb_base: u32,

    // 其他字段...
    // 这个结构很长，这里只定义必要的部分
}

impl BootParams {
    /// 创建新的启动参数
    pub fn new() -> Self {
        Self {
            orig_x: 0,
            orig_y: 0,
            ext_mem_k: 0,
            orig_video_page: 0,
            orig_video_mode: 3,
            orig_video_cols: 80,
            unused1: 0,
            orig_video_ega_bx: 0,
            unused2: 0,
            orig_video_lines: 25,
            orig_video_is_vga: 1,
            orig_video_points: 16,
            lfb_width: 0,
            lfb_height: 0,
            lfb_depth: 0,
            lfb_base: 0,
            lfb_size: 0,
            cl_magic: 0,
            cl_offset: 0,
            lfb_linelength: 0,
            red_size: 0,
            red_pos: 0,
            green_size: 0,
            green_pos: 0,
            blue_size: 0,
            blue_pos: 0,
            rsvd_size: 0,
            rsvd_pos: 0,
            vesapm_seg: 0,
            vesapm_off: 0,
            pages: 0,
            vesa_attributes: 0,
            capabilities: 0,
            ext_lfb_base: 0,
        }
    }
}

impl Default for BootParams {
    fn default() -> Self {
        Self::new()
    }
}

/// EFI Handover 结构
#[repr(C)]
pub struct EfiHandoverParams {
    pub kernel_start: *const u8,
    pub kernel_size: usize,
    pub initrd_start: *const u8,
    pub initrd_size: usize,
    pub cmdline: *const u8,
    pub cmdline_size: usize,
}

/// EFI stub 加载选项
#[derive(Debug, Clone)]
pub struct EfiStubOptions {
    /// 命令行
    pub cmdline: String,
    /// Initrd 路径 (相对于 ISO 根目录)
    pub initrd_path: String,
}

impl EfiStubOptions {
    /// 创建加载选项
    pub fn new(cmdline: &str, initrd_path: &str) -> Self {
        Self {
            cmdline: cmdline.to_string(),
            initrd_path: initrd_path.to_string(),
        }
    }

    /// 转换为 EFI 加载选项格式
    pub fn to_load_options(&self) -> Vec<u16> {
        // EFI 加载选项格式:
        // initrd=PATH cmdline
        let mut options = String::new();
        options.push_str("initrd=");
        options.push_str(&self.initrd_path);
        options.push(' ');
        options.push_str(&self.cmdline);

        // 转换为 UTF-16LE
        let mut result = Vec::new();
        for c in options.encode_utf16() {
            result.push(c.to_le());
        }
        result.push(0); // null terminator
        result
    }
}
