//! Windows 引导支持
//!
//! 实现 Windows ISO 的启动，这是项目中最复杂的部分。
//!
//! # 挑战
//! Windows 启动后会重新初始化 USB 驱动，导致虚拟设备丢失。
//! 需要特殊的内存补丁来解决此问题。
//!
//! # PRD 对应
//! - 模块 C: Windows 引导 (P1)
//! - 模块 B: Block IO 劫持 (P0)

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use log::info;

pub const VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE: usize = 384;
pub const VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE: usize = 384;
pub const VENTOY_WINDOWS_DATA_RESERVED_SIZE: usize = 250;
pub const VENTOY_WINDOWS_DATA_HEADER_SIZE: usize = 1024;

const AUTO_INSTALL_SCRIPT_OFFSET: usize = 0;
const INJECTION_ARCHIVE_OFFSET: usize =
    AUTO_INSTALL_SCRIPT_OFFSET + VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE;
const WINDOWS11_BYPASS_CHECK_OFFSET: usize =
    INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE;
const AUTO_INSTALL_LEN_OFFSET: usize = WINDOWS11_BYPASS_CHECK_OFFSET + 1;
const WINDOWS11_BYPASS_NRO_OFFSET: usize = AUTO_INSTALL_LEN_OFFSET + 4;
const RESERVED_OFFSET: usize = WINDOWS11_BYPASS_NRO_OFFSET + 1;

/// Ventoy Windows auto-install payload appended after `ventoy_windows_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyWindowsAutoInstall<'a> {
    /// Original template path from `ventoy.json`.
    pub source_path: &'a str,
    /// Template file bytes appended after the 1024-byte runtime header.
    pub data: &'a [u8],
}

/// Input for building Ventoy-compatible Windows runtime data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VentoyWindowsRuntimeDataInput<'a> {
    pub auto_install: Option<VentoyWindowsAutoInstall<'a>>,
    pub injection_archive: Option<&'a str>,
    pub windows11_bypass_check: bool,
    pub windows11_bypass_nro: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyWindowsRuntimeDataError {
    AutoInstallTooLarge,
    OutputReserveFailed,
}

/// Build Ventoy's packed `ventoy_windows_data` buffer.
///
/// Ventoy injects this blob through its modified wimboot helper. The header is
/// packed to 1024 bytes and any selected auto-install template bytes are
/// appended directly after it.
pub fn build_ventoy_windows_runtime_data(
    input: VentoyWindowsRuntimeDataInput<'_>,
) -> Result<Vec<u8>, VentoyWindowsRuntimeDataError> {
    debug_assert_eq!(RESERVED_OFFSET + VENTOY_WINDOWS_DATA_RESERVED_SIZE, 1024);

    let auto_install_len = input
        .auto_install
        .as_ref()
        .map_or(0usize, |auto_install| auto_install.data.len());
    if auto_install_len > u32::MAX as usize {
        return Err(VentoyWindowsRuntimeDataError::AutoInstallTooLarge);
    }

    let total_size = VENTOY_WINDOWS_DATA_HEADER_SIZE
        .checked_add(auto_install_len)
        .ok_or(VentoyWindowsRuntimeDataError::OutputReserveFailed)?;
    let mut out = Vec::new();
    out.try_reserve_exact(total_size)
        .map_err(|_| VentoyWindowsRuntimeDataError::OutputReserveFailed)?;
    out.resize(VENTOY_WINDOWS_DATA_HEADER_SIZE, 0);

    if let Some(auto_install) = input.auto_install {
        copy_ventoy_c_string(
            &mut out[AUTO_INSTALL_SCRIPT_OFFSET..INJECTION_ARCHIVE_OFFSET],
            ventoy_basename(auto_install.source_path),
        );
        out[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
            .copy_from_slice(&(auto_install.data.len() as u32).to_le_bytes());
        out.extend_from_slice(auto_install.data);
    }

    if let Some(injection_archive) = input.injection_archive {
        copy_ventoy_c_string(
            &mut out[INJECTION_ARCHIVE_OFFSET..WINDOWS11_BYPASS_CHECK_OFFSET],
            injection_archive,
        );
    }

    out[WINDOWS11_BYPASS_CHECK_OFFSET] = u8::from(input.windows11_bypass_check);
    out[WINDOWS11_BYPASS_NRO_OFFSET] = u8::from(input.windows11_bypass_nro);

    Ok(out)
}

fn copy_ventoy_c_string(field: &mut [u8], value: &str) {
    if field.is_empty() {
        return;
    }

    let bytes = value.as_bytes();
    let copy_len = core::cmp::min(bytes.len(), field.len() - 1);
    field[..copy_len].copy_from_slice(&bytes[..copy_len]);
}

fn ventoy_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod ventoy_windows_runtime_data_tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn encodes_bypass_flags_and_plugin_metadata() {
        let payload = b"answer";
        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput {
            auto_install: Some(VentoyWindowsAutoInstall {
                source_path: "/answer/autounattend.xml",
                data: payload,
            }),
            injection_archive: Some("/ventoy/inject.zip"),
            windows11_bypass_check: true,
            windows11_bypass_nro: true,
        })
        .expect("runtime data");

        let script = b"autounattend.xml";
        let injection = b"/ventoy/inject.zip";

        assert_eq!(data.len(), VENTOY_WINDOWS_DATA_HEADER_SIZE + payload.len());
        assert_eq!(&data[..script.len()], script);
        assert_eq!(data[script.len()], 0);
        assert_eq!(
            &data[INJECTION_ARCHIVE_OFFSET..INJECTION_ARCHIVE_OFFSET + injection.len()],
            injection
        );
        assert_eq!(data[INJECTION_ARCHIVE_OFFSET + injection.len()], 0);
        assert_eq!(data[WINDOWS11_BYPASS_CHECK_OFFSET], 1);
        assert_eq!(
            u32::from_le_bytes(
                data[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
                    .try_into()
                    .unwrap()
            ),
            payload.len() as u32
        );
        assert_eq!(data[WINDOWS11_BYPASS_NRO_OFFSET], 1);
        assert_eq!(&data[VENTOY_WINDOWS_DATA_HEADER_SIZE..], payload);
    }

    #[test]
    fn truncates_c_string_fields_like_ventoy() {
        let mut auto_path = String::from("/");
        let mut injection = String::new();
        for _ in 0..400 {
            auto_path.push('a');
            injection.push('b');
        }

        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput {
            auto_install: Some(VentoyWindowsAutoInstall {
                source_path: &auto_path,
                data: b"x",
            }),
            injection_archive: Some(&injection),
            ..VentoyWindowsRuntimeDataInput::default()
        })
        .expect("runtime data");

        assert!(data[..VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE - 1]
            .iter()
            .all(|byte| *byte == b'a'));
        assert_eq!(data[VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE - 1], 0);
        assert!(data[INJECTION_ARCHIVE_OFFSET
            ..INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE - 1]
            .iter()
            .all(|byte| *byte == b'b'));
        assert_eq!(
            data[INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE - 1],
            0
        );
    }

    #[test]
    fn omits_auto_install_payload_when_absent() {
        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput::default())
            .expect("runtime data");

        assert_eq!(data.len(), VENTOY_WINDOWS_DATA_HEADER_SIZE);
        assert_eq!(data[WINDOWS11_BYPASS_CHECK_OFFSET], 0);
        assert_eq!(
            u32::from_le_bytes(
                data[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
                    .try_into()
                    .unwrap()
            ),
            0
        );
        assert_eq!(data[WINDOWS11_BYPASS_NRO_OFFSET], 0);
        assert!(data.iter().all(|byte| *byte == 0));
    }
}

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

/// ACPI 表注入
///
/// 用于告诉 Windows 虚拟设备的存在
pub mod acpi {
    /// ACPI 表头
    #[repr(C, packed)]
    pub struct AcpiTableHeader {
        pub signature: [u8; 4],
        pub length: u32,
        pub revision: u8,
        pub checksum: u8,
        pub oem_id: [u8; 6],
        pub oem_table_id: [u8; 8],
        pub oem_revision: u32,
        pub creator_id: u32,
        pub creator_revision: u32,
    }

    impl AcpiTableHeader {
        /// 创建新表头
        pub fn new(signature: &[u8; 4], length: u32) -> Self {
            let mut header = Self {
                signature: *signature,
                length,
                revision: 1,
                checksum: 0,
                oem_id: *b"NEXTBT",
                oem_table_id: *b"NBTBOOT ",
                oem_revision: 1,
                creator_id: 0,
                creator_revision: 1,
            };
            header.checksum = header.calculate_checksum();
            header
        }

        /// 计算校验和
        pub fn calculate_checksum(&self) -> u8 {
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    self as *const Self as *const u8,
                    core::mem::size_of::<Self>(),
                )
            };

            let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            0u8.wrapping_sub(sum)
        }
    }

    /// RSDP (Root System Description Pointer)
    #[repr(C, packed)]
    pub struct Rsdp {
        pub signature: [u8; 8],
        pub checksum: u8,
        pub oem_id: [u8; 6],
        pub revision: u8,
        pub rsdt_address: u32,
        // ACPI 2.0+ 扩展
        pub length: u32,
        pub xsdt_address: u64,
        pub extended_checksum: u8,
        pub reserved: [u8; 3],
    }

    /// 查找 RSDP
    pub fn find_rsdp() -> Option<*const Rsdp> {
        // TODO: 在 UEFI 配置表中查找 ACPI 2.0 RSDP
        None
    }

    /// 注入自定义 SSDT
    pub fn inject_ssdt(_table_data: &[u8]) -> Result<(), &'static str> {
        // TODO: 将 SSDT 添加到 XSDT
        // 这需要修改 ACPI 表，风险较高

        // 替代方案: 使用 UEFI Configuration Table
        Err("Not implemented")
    }
}

/// BCD (Boot Configuration Data) 解析
pub mod bcd {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// BCD 对象类型
    #[derive(Debug, Clone, Copy)]
    pub enum BcdObjectType {
        Application,
        Device,
        Inherit,
        Library,
    }

    /// 简单的 BCD 解析器
    pub fn parse_bcd(data: &[u8]) -> Option<BcdStore> {
        // BCD 是注册表格式的 hive 文件
        // 简化实现: 只解析基本结构

        if data.len() < 4 {
            return None;
        }

        // 检查注册表签名 "regf"
        if &data[0..4] != b"regf" {
            return None;
        }

        Some(BcdStore { _data: Vec::new() })
    }

    /// BCD 存储
    pub struct BcdStore {
        _data: Vec<u8>,
    }

    impl BcdStore {
        /// 获取默认启动项
        pub fn get_default_entry(&self) -> Option<u64> {
            // TODO: 解析 BCD 获取 {default} GUID
            None
        }

        /// 获取启动项描述
        pub fn get_entry_description(&self, _id: u64) -> Option<String> {
            None
        }
    }

    /// BCD 元素类型
    #[derive(Debug, Clone, Copy)]
    pub enum BcdElementType {
        /// 应用路径
        ApplicationPath = 0x1200002,
        /// 设备
        OsDevice = 0x2100001,
        /// OS 文件设备
        OsFileDevice = 0x2200002,
        /// 描述
        Description = 0x1200004,
    }
}

/// Windows PE 头信息
#[derive(Debug, Clone)]
pub struct PeInfo {
    /// 机器类型
    pub machine: u16,
    /// 节数
    pub number_of_sections: u16,
    /// 可选头大小
    pub size_of_optional_header: u16,
    /// 特征
    pub characteristics: u16,
    /// 入口点
    pub entry_point: u32,
    /// 镜像基址
    pub image_base: u64,
    /// 镜像大小
    pub image_size: u32,
    /// 子系统
    pub subsystem: u16,
}

impl PeInfo {
    /// 从 PE 文件解析信息
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 0x40 {
            return None;
        }

        // 检查 DOS 签名
        if &data[0..2] != b"MZ" {
            return None;
        }

        // 获取 PE 头偏移
        let pe_offset =
            u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;

        if pe_offset + 24 > data.len() {
            return None;
        }

        // 检查 PE 签名
        if &data[pe_offset..pe_offset + 4] != b"PE\x00\x00" {
            return None;
        }

        // 解析 COFF 头
        let machine = u16::from_le_bytes([data[pe_offset + 4], data[pe_offset + 5]]);
        let number_of_sections = u16::from_le_bytes([data[pe_offset + 6], data[pe_offset + 7]]);
        let size_of_optional_header =
            u16::from_le_bytes([data[pe_offset + 20], data[pe_offset + 21]]);
        let characteristics = u16::from_le_bytes([data[pe_offset + 22], data[pe_offset + 23]]);

        // 解析可选头
        let opt_offset = pe_offset + 24;
        if opt_offset + size_of_optional_header as usize > data.len() {
            return None;
        }

        let magic = u16::from_le_bytes([data[opt_offset], data[opt_offset + 1]]);

        let (entry_point, image_base, image_size, subsystem) = if magic == 0x10B {
            // PE32
            let entry = u32::from_le_bytes([
                data[opt_offset + 16],
                data[opt_offset + 17],
                data[opt_offset + 18],
                data[opt_offset + 19],
            ]);
            let base = u32::from_le_bytes([
                data[opt_offset + 28],
                data[opt_offset + 29],
                data[opt_offset + 30],
                data[opt_offset + 31],
            ]) as u64;
            let size = u32::from_le_bytes([
                data[opt_offset + 56],
                data[opt_offset + 57],
                data[opt_offset + 58],
                data[opt_offset + 59],
            ]);
            let sub = u16::from_le_bytes([data[opt_offset + 68], data[opt_offset + 69]]);
            (entry, base, size, sub)
        } else if magic == 0x20B {
            // PE32+
            let entry = u32::from_le_bytes([
                data[opt_offset + 16],
                data[opt_offset + 17],
                data[opt_offset + 18],
                data[opt_offset + 19],
            ]);
            let base = u64::from_le_bytes([
                data[opt_offset + 24],
                data[opt_offset + 25],
                data[opt_offset + 26],
                data[opt_offset + 27],
                data[opt_offset + 28],
                data[opt_offset + 29],
                data[opt_offset + 30],
                data[opt_offset + 31],
            ]);
            let size = u32::from_le_bytes([
                data[opt_offset + 56],
                data[opt_offset + 57],
                data[opt_offset + 58],
                data[opt_offset + 59],
            ]);
            let sub = u16::from_le_bytes([data[opt_offset + 68], data[opt_offset + 69]]);
            (entry, base, size, sub)
        } else {
            return None;
        };

        Some(Self {
            machine,
            number_of_sections,
            size_of_optional_header,
            characteristics,
            entry_point,
            image_base,
            image_size,
            subsystem,
        })
    }

    /// 检查是否为 EFI 应用
    pub fn is_efi_application(&self) -> bool {
        self.subsystem == 10 || self.subsystem == 11 // EFI application or EFI boot service driver
    }
}

/// 从 ISO 文件列表检测是否为 Windows ISO
pub fn is_windows_iso(files: &[&str]) -> bool {
    files.iter().any(|f| {
        let f_lower = f.to_lowercase();
        f_lower.contains("bootmgfw.efi")
            || f_lower.contains("install.wim")
            || f_lower.contains("install.esd")
            || f_lower.contains("boot.sdi")
    })
}
