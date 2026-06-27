use alloc::string::{String, ToString};
use alloc::vec::Vec;
use log::info;

/// Windows 启动方法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsBootMethod {
    /// 标准 UEFI 启动 (bootmgfw.efi)
    Standard,
    /// WinPE 模式 (boot.sdi)
    WinPE,
    /// WIM 直接启动
    WimBoot,
    /// VHD 启动
    VhdBoot,
}

impl WindowsBootMethod {
    /// 获取显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            WindowsBootMethod::Standard => "Standard UEFI Boot",
            WindowsBootMethod::WinPE => "WinPE",
            WindowsBootMethod::WimBoot => "WIM Boot",
            WindowsBootMethod::VhdBoot => "VHD Boot",
        }
    }
}

/// Windows 启动配置
#[derive(Debug, Clone)]
pub struct WindowsBootConfig {
    /// 启动方法
    pub method: WindowsBootMethod,
    /// bootmgfw.efi 路径
    pub bootmgfw_path: String,
    /// BCD 存储路径
    pub bcd_path: String,
    /// install.wim 路径 (可选)
    pub wim_path: Option<String>,
    /// ISO 挂载路径
    pub iso_mount_point: String,
    /// Windows 版本
    pub windows_version: WindowsVersion,
}

impl WindowsBootConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self {
            method: WindowsBootMethod::Standard,
            bootmgfw_path: "/efi/microsoft/boot/bootmgfw.efi".to_string(),
            bcd_path: "/efi/microsoft/boot/bcd".to_string(),
            wim_path: None,
            iso_mount_point: String::new(),
            windows_version: WindowsVersion::Unknown,
        }
    }

    /// 检测 Windows 版本
    pub fn detect_windows_version(files: &[&str]) -> WindowsVersion {
        for file in files {
            let f = file.to_lowercase();

            // Windows 11
            if f.contains("sources/install.wim") || f.contains("sources/install.esd") {
                // 需要解析 WIM 获取版本
                // 简化: 检查文件名
                if f.contains("win11") || f.contains("windows11") {
                    return WindowsVersion::Windows11;
                }
                if f.contains("win10") || f.contains("windows10") {
                    return WindowsVersion::Windows10;
                }
                if f.contains("win8") || f.contains("windows8") {
                    return WindowsVersion::Windows8_1;
                }

                // 默认假设是 Windows 10+
                return WindowsVersion::Windows10;
            }

            // WinPE
            if f.contains("boot.sdi") {
                return WindowsVersion::WinPE;
            }
        }

        WindowsVersion::Unknown
    }

    /// 从 ISO 文件列表创建配置
    pub fn from_iso_files(files: &[&str]) -> Self {
        let version = Self::detect_windows_version(files);

        let method = if version == WindowsVersion::WinPE {
            WindowsBootMethod::WinPE
        } else {
            WindowsBootMethod::Standard
        };

        let wim_path = files
            .iter()
            .find(|f| {
                let f_lower = f.to_lowercase();
                f_lower.contains("sources/install.wim") || f_lower.contains("sources/install.esd")
            })
            .map(|s| s.to_string());

        Self {
            method,
            bootmgfw_path: "/efi/microsoft/boot/bootmgfw.efi".to_string(),
            bcd_path: "/efi/microsoft/boot/bcd".to_string(),
            wim_path,
            iso_mount_point: String::new(),
            windows_version: version,
        }
    }
}

impl Default for WindowsBootConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Windows 版本
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsVersion {
    Windows11,
    Windows10,
    Windows8_1,
    Windows8,
    Windows7,
    WinPE,
    Unknown,
}

impl WindowsVersion {
    /// 获取显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            WindowsVersion::Windows11 => "Windows 11",
            WindowsVersion::Windows10 => "Windows 10",
            WindowsVersion::Windows8_1 => "Windows 8.1",
            WindowsVersion::Windows8 => "Windows 8",
            WindowsVersion::Windows7 => "Windows 7",
            WindowsVersion::WinPE => "WinPE",
            WindowsVersion::Unknown => "Windows",
        }
    }
}

/// Windows 启动器
pub struct WindowsBootloader {
    config: WindowsBootConfig,
    bootmgfw_data: Option<Vec<u8>>,
}

impl WindowsBootloader {
    /// 创建新的启动器
    pub fn new(config: WindowsBootConfig) -> Self {
        Self {
            config,
            bootmgfw_data: None,
        }
    }

    /// 加载 bootmgfw.efi
    pub fn load_bootmgfw(&mut self, data: Vec<u8>) -> Result<(), WindowsBootError> {
        // 验证 PE 魔数
        if data.len() < 0x40 {
            return Err(WindowsBootError::InvalidBootFile);
        }

        // 检查 DOS 签名 "MZ"
        if &data[0..2] != b"MZ" {
            return Err(WindowsBootError::InvalidBootFile);
        }

        // 检查 PE 签名
        let pe_offset =
            u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
        if pe_offset + 4 > data.len() {
            return Err(WindowsBootError::InvalidBootFile);
        }

        if &data[pe_offset..pe_offset + 4] != b"PE\x00\x00" {
            return Err(WindowsBootError::InvalidBootFile);
        }

        info!("Loaded bootmgfw.efi: {} bytes", data.len());
        self.bootmgfw_data = Some(data);
        Ok(())
    }

    /// 准备启动环境
    pub fn prepare(&mut self) -> Result<(), WindowsBootError> {
        info!("Preparing Windows boot environment...");

        // 1. 设置虚拟 Block IO
        self.setup_virtual_block_io()?;

        // 2. 注入必要的驱动 (如果需要)
        // self.inject_drivers()?;

        // 3. 修改 BCD (如果需要)
        // self.modify_bcd()?;

        Ok(())
    }

    /// 设置虚拟 Block IO
    fn setup_virtual_block_io(&mut self) -> Result<(), WindowsBootError> {
        // 关键点:
        // 1. 设备类型必须是 CD-ROM 或 HDD
        // 2. 必须在 bootmgfw.efi 加载前注册
        // 3. 需要处理 4K/512B 扇区问题

        info!("Setting up virtual Block IO...");

        // TODO: 调用 nextboot-virtio 创建虚拟设备
        // 需要创建一个 CD-ROM 类型的虚拟设备

        Ok(())
    }

    /// 执行启动
    ///
    /// # 安全性
    /// 此函数将控制权转交给 Windows Boot Manager
    pub unsafe fn boot(self) -> ! {
        info!("Booting Windows...");

        // TODO: 实现 Windows 启动
        // 流程:
        // 1. 确保虚拟 Block IO 已注册
        // 2. 加载 bootmgfw.efi
        // 3. 设置适当的设备路径
        // 4. 调用 UEFI LoadImage
        // 5. 调用 StartImage

        loop {
            core::hint::spin_loop();
        }
    }

    /// 获取配置
    pub fn config(&self) -> &WindowsBootConfig {
        &self.config
    }

    /// 检查是否准备好启动
    pub fn is_ready(&self) -> bool {
        self.bootmgfw_data.is_some()
    }
}

/// Windows 启动错误
#[derive(Debug, Clone, Copy)]
pub enum WindowsBootError {
    /// 虚拟设备创建失败
    VirtualDeviceFailed,
    /// 驱动注入失败
    DriverInjectionFailed,
    /// BCD 修改失败
    BcdModificationFailed,
    /// 启动文件未找到
    BootFileNotFound,
    /// 内存不足
    OutOfMemory,
    /// 无效的启动文件
    InvalidBootFile,
    /// UEFI 服务不可用
    UefiNotAvailable,
    /// 不支持的 Windows 版本
    UnsupportedVersion,
}

impl core::fmt::Display for WindowsBootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WindowsBootError::VirtualDeviceFailed => write!(f, "Failed to create virtual device"),
            WindowsBootError::DriverInjectionFailed => write!(f, "Failed to inject drivers"),
            WindowsBootError::BcdModificationFailed => write!(f, "Failed to modify BCD"),
            WindowsBootError::BootFileNotFound => write!(f, "Boot file not found"),
            WindowsBootError::OutOfMemory => write!(f, "Out of memory"),
            WindowsBootError::InvalidBootFile => write!(f, "Invalid boot file"),
            WindowsBootError::UefiNotAvailable => write!(f, "UEFI services not available"),
            WindowsBootError::UnsupportedVersion => write!(f, "Unsupported Windows version"),
        }
    }
}
