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
