use alloc::string::{String, ToString};

/// 菜单项
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// 显示名称
    pub label: String,
    /// 文件路径
    pub path: String,
    /// 文件大小
    pub size: u64,
    /// 检测到的类型
    pub iso_type: IsoType,
}

impl MenuItem {
    /// 创建新的菜单项
    pub fn new(label: String, path: String, size: u64, iso_type: IsoType) -> Self {
        Self {
            label,
            path,
            size,
            iso_type,
        }
    }

    /// 从文件信息创建
    pub fn from_file_info(path: &str, size: u64) -> Self {
        let label = path.split('/').last().unwrap_or(path).to_string();

        let iso_type = IsoType::detect_from_path(path);

        Self {
            label,
            path: path.to_string(),
            size,
            iso_type,
        }
    }
}

/// ISO 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoType {
    /// Windows 安装镜像
    Windows,
    /// Ubuntu
    Ubuntu,
    /// Debian
    Debian,
    /// Fedora
    Fedora,
    /// Arch Linux
    Arch,
    /// 通用 Linux
    GenericLinux,
    /// PE 环境
    WinPE,
    /// 未知类型
    Unknown,
}

impl IsoType {
    /// 获取显示图标
    pub fn icon(&self) -> &'static str {
        match self {
            IsoType::Windows => "[W]",
            IsoType::Ubuntu => "[U]",
            IsoType::Debian => "[D]",
            IsoType::Fedora => "[F]",
            IsoType::Arch => "[A]",
            IsoType::GenericLinux => "[L]",
            IsoType::WinPE => "[P]",
            IsoType::Unknown => "[?]",
        }
    }

    /// 获取显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            IsoType::Windows => "Windows",
            IsoType::Ubuntu => "Ubuntu",
            IsoType::Debian => "Debian",
            IsoType::Fedora => "Fedora",
            IsoType::Arch => "Arch Linux",
            IsoType::GenericLinux => "Linux",
            IsoType::WinPE => "WinPE",
            IsoType::Unknown => "Unknown",
        }
    }

    /// 根据 ISO 内容检测类型
    pub fn detect(files: &[&str]) -> Self {
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("bootmgfw.efi")
                || f_lower.contains("install.wim")
                || f_lower.contains("install.esd")
        }) {
            return IsoType::Windows;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("boot.sdi") && f_lower.contains("winpe")
        }) {
            return IsoType::WinPE;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("casper/vmlinuz") || f_lower.contains(".disk/info")
        }) {
            return IsoType::Ubuntu;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("install.amd") || f_lower.contains("install.386")
        }) {
            return IsoType::Debian;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("images/pxeboot") || f_lower.contains("fedora")
        }) {
            return IsoType::Fedora;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("arch/boot")
        }) {
            return IsoType::Arch;
        }

        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("vmlinuz")
                || f_lower.contains("initrd")
                || f_lower.contains("grub.cfg")
        }) {
            return IsoType::GenericLinux;
        }

        IsoType::Unknown
    }

    /// 从路径检测类型
    pub fn detect_from_path(path: &str) -> Self {
        let path_lower = path.to_lowercase();

        if path_lower.contains("windows") {
            return IsoType::Windows;
        }
        if path_lower.contains("ubuntu") {
            return IsoType::Ubuntu;
        }
        if path_lower.contains("debian") {
            return IsoType::Debian;
        }
        if path_lower.contains("fedora") {
            return IsoType::Fedora;
        }
        if path_lower.contains("arch") {
            return IsoType::Arch;
        }
        if path_lower.contains("winpe") || path_lower.contains("pe_") {
            return IsoType::WinPE;
        }
        if path_lower.contains("linux") {
            return IsoType::GenericLinux;
        }

        IsoType::Unknown
    }
}
