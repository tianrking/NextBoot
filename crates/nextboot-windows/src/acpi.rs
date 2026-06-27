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
