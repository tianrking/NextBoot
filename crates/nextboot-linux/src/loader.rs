use alloc::vec::Vec;
use log::{info, warn};

use crate::LinuxBootConfig;

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
            warn!(
                "Unknown initrd format, first bytes: {:02X?}",
                &data[0..8.min(data.len())]
            );
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

    /// 拆回已验证的启动输入
    pub fn into_parts(self) -> (LinuxBootConfig, Vec<u8>, Vec<u8>) {
        (self.config, self.kernel_data, self.initrd_data)
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
