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
