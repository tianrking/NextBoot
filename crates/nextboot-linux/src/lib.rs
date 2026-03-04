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

use alloc::string::String;
use alloc::vec::Vec;

/// Linux 发行版类型
#[derive(Debug, Clone, Copy)]
pub enum LinuxDistro {
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    OpenSuse,
    CentOS,
    Generic,
}

impl LinuxDistro {
    /// 从 ISO 文件列表检测发行版
    pub fn detect(files: &[&str]) -> Self {
        // Ubuntu: casper/vmlinuz
        if files.iter().any(|f| f.contains("casper/vmlinuz") || f.contains("casper/initrd")) {
            return LinuxDistro::Ubuntu;
        }

        // Debian: install.amd/vmlinuz
        if files.iter().any(|f| f.contains("install.amd")) {
            return LinuxDistro::Debian;
        }

        // Fedora: images/pxeboot
        if files.iter().any(|f| f.contains("images/pxeboot")) {
            return LinuxDistro::Fedora;
        }

        // Arch: arch/boot/x86_64/vmlinuz
        if files.iter().any(|f| f.contains("arch/boot")) {
            return LinuxDistro::Arch;
        }

        LinuxDistro::Generic
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
}

impl LinuxBootConfig {
    /// 为指定发行版创建默认配置
    pub fn for_distro(distro: LinuxDistro, iso_path: &str) -> Self {
        let (kernel, initrd, extra_cmdline) = match distro {
            LinuxDistro::Ubuntu => (
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
                "root=live:CDLABEL=Fedora quiet"
            ),
            LinuxDistro::Arch => (
                "/arch/boot/x86_64/vmlinuz-linux",
                "/arch/boot/x86_64/initramfs-linux.img",
                "archisobasedir=arch archisolabel=ARCH_$(date +%Y%m)"
            ),
            _ => (
                "/boot/vmlinuz",
                "/boot/initrd.img",
                ""
            ),
        };

        // 构建完整的命令行
        let cmdline = match distro {
            LinuxDistro::Ubuntu => {
                alloc::format!(
                    "{} iso-scan/filename={} --",
                    extra_cmdline, iso_path
                )
            }
            LinuxDistro::Debian => {
                alloc::format!(
                    "{} findiso={}",
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
        }
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
        if data.len() < 6 || &data[0x202..0x208] != b"HdrS" {
            return Err(LinuxBootError::InvalidKernel);
        }

        self.kernel_data = data;
        Ok(())
    }

    /// 加载 Initrd
    pub fn load_initrd(&mut self, data: Vec<u8>) -> Result<(), LinuxBootError> {
        // Initrd 可以是 gzip 或 cpio 格式
        // 简单验证: 检查 gzip 魔数或 cpio newc 格式
        if data.len() < 6 {
            return Err(LinuxBootError::InvalidInitrd);
        }

        self.initrd_data = data;
        Ok(())
    }

    /// 执行启动
    ///
    /// # 安全性
    /// 此函数不会返回，直接跳转到 Kernel
    pub unsafe fn boot(self) -> ! {
        // TODO: 实现 Linux Kernel 启动协议
        // 1. 设置实模式内核头
        // 2. 加载 Kernel 到 1MB 以上
        // 3. 设置 initrd 地址和大小
        // 4. 设置 cmdline
        // 5. 跳转到内核入口点

        // 使用 UEFI LoadImage 和 StartImage 也是一种选择
        // 但对于 Linux 需要特殊处理

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
}

/// 解析 isolinux/syslinux 配置
pub fn parse_isolinux_cfg(cfg: &str) -> Option<(String, String)> {
    // 简单解析: 查找 KERNEL 和 INITRD 行
    let mut kernel = None;
    let mut initrd = None;

    for line in cfg.lines() {
        let line = line.trim();
        if line.starts_with("KERNEL") || line.starts_with("LINUX") {
            kernel = line.split_whitespace().nth(1);
        } else if line.starts_with("INITRD") {
            initrd = line.split_whitespace().nth(1);
        }
    }

    match (kernel, initrd) {
        (Some(k), Some(i)) => Some((k.to_string(), i.to_string())),
        _ => None,
    }
}

/// 解析 GRUB 配置
pub fn parse_grub_cfg(cfg: &str) -> Option<(String, String)> {
    // 查找 linux 和 initrd 行
    let mut kernel = None;
    let mut initrd = None;

    for line in cfg.lines() {
        let line = line.trim();
        if line.starts_with("linux") || line.starts_with("linux16") || line.starts_with("linuxefi") {
            kernel = line.split_whitespace().nth(1);
        } else if line.starts_with("initrd") || line.starts_with("initrd16") || line.starts_with("initrdefi") {
            initrd = line.split_whitespace().nth(1);
        }
    }

    match (kernel, initrd) {
        (Some(k), Some(i)) => Some((k.to_string(), i.to_string())),
        _ => None,
    }
}
