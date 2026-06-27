use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Linux 内核启动参数结构
#[repr(C, packed)]
pub struct BootParams {
    // 屏幕信息
    pub orig_x: u8,
    pub orig_y: u8,
    pub ext_mem_k: u16,
    pub orig_video_page: u16,
    pub orig_video_mode: u8,
    pub orig_video_cols: u8,
    pub unused1: u16,
    pub orig_video_ega_bx: u16,
    pub unused2: u16,
    pub orig_video_lines: u8,
    pub orig_video_is_vga: u8,
    pub orig_video_points: u16,

    // VESA 信息
    pub lfb_width: u16,
    pub lfb_height: u16,
    pub lfb_depth: u16,
    pub lfb_base: u32,
    pub lfb_size: u32,
    pub cl_magic: u16,
    pub cl_offset: u16,
    pub lfb_linelength: u16,
    pub red_size: u8,
    pub red_pos: u8,
    pub green_size: u8,
    pub green_pos: u8,
    pub blue_size: u8,
    pub blue_pos: u8,
    pub rsvd_size: u8,
    pub rsvd_pos: u8,
    pub vesapm_seg: u16,
    pub vesapm_off: u16,
    pub pages: u16,
    pub vesa_attributes: u16,
    pub capabilities: u32,
    pub ext_lfb_base: u32,
    // 其他字段...
    // 这个结构很长，这里只定义必要的部分
}

impl BootParams {
    /// 创建新的启动参数
    pub fn new() -> Self {
        Self {
            orig_x: 0,
            orig_y: 0,
            ext_mem_k: 0,
            orig_video_page: 0,
            orig_video_mode: 3,
            orig_video_cols: 80,
            unused1: 0,
            orig_video_ega_bx: 0,
            unused2: 0,
            orig_video_lines: 25,
            orig_video_is_vga: 1,
            orig_video_points: 16,
            lfb_width: 0,
            lfb_height: 0,
            lfb_depth: 0,
            lfb_base: 0,
            lfb_size: 0,
            cl_magic: 0,
            cl_offset: 0,
            lfb_linelength: 0,
            red_size: 0,
            red_pos: 0,
            green_size: 0,
            green_pos: 0,
            blue_size: 0,
            blue_pos: 0,
            rsvd_size: 0,
            rsvd_pos: 0,
            vesapm_seg: 0,
            vesapm_off: 0,
            pages: 0,
            vesa_attributes: 0,
            capabilities: 0,
            ext_lfb_base: 0,
        }
    }
}

impl Default for BootParams {
    fn default() -> Self {
        Self::new()
    }
}

/// EFI Handover 结构
#[repr(C)]
pub struct EfiHandoverParams {
    pub kernel_start: *const u8,
    pub kernel_size: usize,
    pub initrd_start: *const u8,
    pub initrd_size: usize,
    pub cmdline: *const u8,
    pub cmdline_size: usize,
}

/// EFI stub 加载选项
#[derive(Debug, Clone)]
pub struct EfiStubOptions {
    /// 命令行
    pub cmdline: String,
    /// Initrd 路径 (相对于 ISO 根目录)
    pub initrd_path: String,
}

impl EfiStubOptions {
    /// 创建加载选项
    pub fn new(cmdline: &str, initrd_path: &str) -> Self {
        Self {
            cmdline: cmdline.to_string(),
            initrd_path: initrd_path.to_string(),
        }
    }

    /// 转换为 EFI 加载选项格式
    pub fn to_load_option_string(&self) -> String {
        let initrd_path = normalize_efi_stub_path(&self.initrd_path);

        match (initrd_path.is_empty(), self.cmdline.is_empty()) {
            (true, true) => String::new(),
            (true, false) => self.cmdline.clone(),
            (false, true) => format!("initrd={}", initrd_path),
            (false, false) => format!("initrd={} {}", initrd_path, self.cmdline),
        }
    }

    /// 转换为 EFI UTF-16 加载选项格式
    pub fn to_load_options(&self) -> Vec<u16> {
        let options = self.to_load_option_string();

        // 转换为 UTF-16LE
        let mut result = Vec::new();
        for c in options.encode_utf16() {
            result.push(c.to_le());
        }
        result.push(0); // null terminator
        result
    }
}

fn normalize_efi_stub_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut normalized = String::new();
    if !path.starts_with('/') && !path.starts_with('\\') {
        normalized.push('\\');
    }

    for ch in path.chars() {
        normalized.push(if ch == '/' { '\\' } else { ch });
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::EfiStubOptions;

    #[test]
    fn efi_stub_load_options_normalize_initrd_path() {
        let options = EfiStubOptions::new("boot=casper quiet", "/casper/initrd");
        assert_eq!(
            options.to_load_option_string(),
            "initrd=\\casper\\initrd boot=casper quiet"
        );
    }

    #[test]
    fn efi_stub_load_options_add_leading_separator() {
        let options = EfiStubOptions::new("", "images/pxeboot/initrd.img");
        assert_eq!(
            options.to_load_option_string(),
            "initrd=\\images\\pxeboot\\initrd.img"
        );
    }
}
