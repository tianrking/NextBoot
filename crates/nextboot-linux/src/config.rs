use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use crate::LinuxDistro;

/// Linux 启动配置
#[derive(Debug, Clone)]
pub struct LinuxBootConfig {
    /// 发行版类型
    pub distro: LinuxDistro,
    /// Kernel 文件路径
    pub kernel_path: String,
    /// Initrd 文件路径
    pub initrd_path: String,
    /// Initrd 文件路径链，按 GRUB/ISOLINUX 声明顺序拼接。
    pub initrd_paths: Vec<String>,
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
                "boot=casper quiet splash maybe-ubiquity",
            ),
            LinuxDistro::LinuxMint => (
                "/casper/vmlinuz",
                "/casper/initrd",
                "boot=casper quiet splash",
            ),
            LinuxDistro::PopOs => (
                "/casper/vmlinuz",
                "/casper/initrd",
                "boot=casper quiet splash",
            ),
            LinuxDistro::Debian => (
                "/install.amd/vmlinuz",
                "/install.amd/initrd.gz",
                "vga=788 -- quiet",
            ),
            LinuxDistro::Fedora => (
                "/images/pxeboot/vmlinuz",
                "/images/pxeboot/initrd.img",
                "root=live:CDLABEL=Fedora quiet rhgb",
            ),
            LinuxDistro::Arch => (
                "/arch/boot/x86_64/vmlinuz-linux",
                "/arch/boot/x86_64/initramfs-linux.img",
                "archisobasedir=arch archisolabel=ARCH_$(date +%Y%m)",
            ),
            LinuxDistro::Manjaro => (
                "/boot/vmlinuz-x86_64",
                "/boot/initramfs-x86_64.img",
                "driver=free tz=utc lang=en_US keytable=us",
            ),
            LinuxDistro::OpenSuse => (
                "/boot/x86_64/loader/linux",
                "/boot/x86_64/loader/initrd",
                "install=cd:/ quiet",
            ),
            LinuxDistro::CentOS => (
                "/images/pxeboot/vmlinuz",
                "/images/pxeboot/initrd.img",
                "inst.stage2=hd:LABEL=CentOS quiet",
            ),
            LinuxDistro::Generic => ("/boot/vmlinuz", "/boot/initrd.img", ""),
        };

        // 构建完整的命令行
        let cmdline = match distro {
            LinuxDistro::Ubuntu | LinuxDistro::LinuxMint | LinuxDistro::PopOs => {
                format!("{} iso-scan/filename={} --", extra_cmdline, iso_path)
            }
            LinuxDistro::Debian => {
                format!("{} findiso={}", extra_cmdline, iso_path)
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

        let initrd_path = initrd.to_string();

        Self {
            distro,
            kernel_path: kernel.to_string(),
            initrd_path: initrd_path.clone(),
            initrd_paths: vec![initrd_path],
            cmdline,
            iso_path: iso_path.to_string(),
            use_efi: true,
        }
    }

    /// 从已发现的 Kernel/Initrd 路径创建启动配置。
    pub fn from_paths(
        distro: LinuxDistro,
        iso_path: &str,
        kernel_path: &str,
        initrd_path: &str,
        cmdline: &str,
    ) -> Self {
        Self::from_initrd_paths(
            distro,
            iso_path,
            kernel_path,
            vec![initrd_path.to_string()],
            cmdline,
        )
    }

    /// 从已发现的 Kernel/Initrd 路径链创建启动配置。
    pub fn from_initrd_paths(
        distro: LinuxDistro,
        iso_path: &str,
        kernel_path: &str,
        initrd_paths: Vec<String>,
        cmdline: &str,
    ) -> Self {
        let initrd_path = initrd_paths.last().cloned().unwrap_or_default();
        Self {
            distro,
            kernel_path: kernel_path.to_string(),
            initrd_path,
            initrd_paths,
            cmdline: cmdline.to_string(),
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

/// 从 ISO 文件列表自动检测启动配置
pub fn auto_detect_config(files: &[&str], iso_path: &str) -> Option<LinuxBootConfig> {
    let distro = LinuxDistro::detect(files);
    Some(LinuxBootConfig::for_distro(distro, iso_path))
}
