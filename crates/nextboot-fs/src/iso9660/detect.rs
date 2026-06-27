/// 检测 ISO 中的操作系统类型
pub fn detect_os_type(files: &[&str]) -> IsoOsType {
    for file in files {
        let file_lower = file.to_lowercase();

        // Windows
        if file_lower.contains("bootmgfw.efi") || file_lower.contains("install.wim") {
            return IsoOsType::Windows;
        }

        // Ubuntu
        if file_lower.contains("casper/vmlinuz") || file_lower.contains(".disk/info") {
            return IsoOsType::Ubuntu;
        }

        // Debian
        if file_lower.contains("install.amd") {
            return IsoOsType::Debian;
        }

        // Fedora
        if file_lower.contains("images/pxeboot") {
            return IsoOsType::Fedora;
        }

        // Arch
        if file_lower.contains("arch/boot") {
            return IsoOsType::Arch;
        }

        // 通用 Linux
        if file_lower.contains("vmlinuz") || file_lower.contains("initrd") {
            return IsoOsType::GenericLinux;
        }
    }

    IsoOsType::Unknown
}

/// ISO 操作系统类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoOsType {
    Windows,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    GenericLinux,
    Unknown,
}
