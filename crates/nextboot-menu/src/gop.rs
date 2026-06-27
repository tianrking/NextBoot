//! UEFI GOP (Graphics Output Protocol) 封装

use crate::font::get_font_bitmap;
use alloc::vec::Vec;
use uefi::proto::console::gop::{
    BltOp, BltPixel, BltRegion, FrameBuffer, GraphicsOutput, PixelFormat,
};
use uefi::table::boot::{BootServices, ScopedProtocol, SearchType};
use uefi::Identify;

/// 颜色 (RGBA)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::new_const(0, 0, 0);
    pub const WHITE: Self = Self::new_const(255, 255, 255);
    pub const RED: Self = Self::new_const(255, 0, 0);
    pub const GREEN: Self = Self::new_const(0, 255, 0);
    pub const BLUE: Self = Self::new_const(0, 0, 255);
    pub const CYAN: Self = Self::new_const(0, 255, 255);
    pub const MAGENTA: Self = Self::new_const(255, 0, 255);
    pub const YELLOW: Self = Self::new_const(255, 255, 0);
    pub const GRAY: Self = Self::new_const(128, 128, 128);
    pub const DARK_GRAY: Self = Self::new_const(64, 64, 64);
    pub const LIGHT_GRAY: Self = Self::new_const(192, 192, 192);
    pub const ORANGE: Self = Self::new_const(255, 165, 0);

    const fn new_const(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 创建新颜色
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self::new_const(r, g, b)
    }

    /// 创建带透明度的颜色
    pub fn with_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 从 RGB 值创建
    pub fn from_rgb(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xFF) as u8,
            g: ((rgb >> 8) & 0xFF) as u8,
            b: (rgb & 0xFF) as u8,
            a: 255,
        }
    }

    /// 转换为 RGB 值
    pub fn to_rgb(&self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// 转换为 U32 (根据像素格式)
    pub fn to_u32(&self, format: PixelFormat) -> u32 {
        match format {
            PixelFormat::Rgb => ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32,
            PixelFormat::Bgr => ((self.b as u32) << 16) | ((self.g as u32) << 8) | self.r as u32,
            PixelFormat::Bitmask | PixelFormat::BltOnly => self.to_rgb(),
        }
    }

    /// 转换为 BltPixel
    pub fn to_blt_pixel(&self) -> BltPixel {
        BltPixel::new(self.r, self.g, self.b)
    }

    /// 混合两个颜色
    pub fn blend(&self, other: &Color) -> Color {
        if self.a == 255 {
            return *self;
        }
        if self.a == 0 {
            return *other;
        }

        let alpha = self.a as u16;
        let inv_alpha = 255 - self.a as u16;

        Color {
            r: ((self.r as u16 * alpha + other.r as u16 * inv_alpha) / 255) as u8,
            g: ((self.g as u16 * alpha + other.g as u16 * inv_alpha) / 255) as u8,
            b: ((self.b as u16 * alpha + other.b as u16 * inv_alpha) / 255) as u8,
            a: 255,
        }
    }

    /// 调整亮度
    pub fn adjust_brightness(&self, factor: f32) -> Color {
        Color {
            r: ((self.r as f32 * factor).min(255.0)) as u8,
            g: ((self.g as f32 * factor).min(255.0)) as u8,
            b: ((self.b as f32 * factor).min(255.0)) as u8,
            a: self.a,
        }
    }
}

/// GOP 上下文
pub struct GopContext<'a> {
    gop: ScopedProtocol<'a, GraphicsOutput>,
    width: usize,
    height: usize,
    format: PixelFormat,
    stride: usize,
}

impl<'a> GopContext<'a> {
    /// 初始化 GOP
    pub fn init(bt: &'a BootServices) -> uefi::Result<Self> {
        let gop_handles = bt.locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutput::GUID))?;
        let gop_handle = gop_handles.first().ok_or(uefi::Status::UNSUPPORTED)?;

        let gop = bt.open_protocol_exclusive::<GraphicsOutput>(*gop_handle)?;
        let format = gop.current_mode_info().pixel_format();
        let info = gop.current_mode_info();
        let (width, height) = info.resolution();
        let stride = info.stride();

        Ok(Self {
            gop,
            width,
            height,
            format,
            stride,
        })
    }

    /// 获取帧缓冲区
    pub fn frame_buffer(&mut self) -> FrameBuffer<'_> {
        self.gop.frame_buffer()
    }

    /// 获取分辨率
    pub fn resolution(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// 获取像素格式
    pub fn pixel_format(&self) -> PixelFormat {
        self.format
    }

    /// 绘制像素
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        let mut fb = self.gop.frame_buffer();
        let pixel_size = 4;
        let offset = (y * self.stride + x) * pixel_size;

        if offset + pixel_size <= fb.size() {
            let pixel = color.to_u32(self.format);
            for (i, byte) in pixel.to_le_bytes().iter().enumerate() {
                unsafe {
                    fb.write_byte(offset + i, *byte);
                }
            }
        }
    }

    /// 读取像素
    pub fn read_pixel(&mut self, x: usize, y: usize) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let fb = self.gop.frame_buffer();
        let pixel_size = 4;
        let offset = (y * self.stride + x) * pixel_size;
        if offset + pixel_size > fb.size() {
            return None;
        }

        let bytes = [
            unsafe { fb.read_byte(offset) },
            unsafe { fb.read_byte(offset + 1) },
            unsafe { fb.read_byte(offset + 2) },
            unsafe { fb.read_byte(offset + 3) },
        ];
        let pixel = u32::from_le_bytes(bytes);
        let (r, g, b) = match self.format {
            PixelFormat::Bgr => (
                (pixel & 0xFF) as u8,
                ((pixel >> 8) & 0xFF) as u8,
                ((pixel >> 16) & 0xFF) as u8,
            ),
            PixelFormat::Rgb | PixelFormat::Bitmask | PixelFormat::BltOnly => (
                ((pixel >> 16) & 0xFF) as u8,
                ((pixel >> 8) & 0xFF) as u8,
                (pixel & 0xFF) as u8,
            ),
        };

        Some(Color::new(r, g, b))
    }

    /// 填充矩形
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let end_x = (x + w).min(self.width);
        let end_y = (y + h).min(self.height);

        for py in y..end_y {
            for px in x..end_x {
                self.draw_pixel(px, py, color);
            }
        }
    }

    /// 清屏
    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// 绘制水平线
    pub fn draw_hline(&mut self, x: usize, y: usize, length: usize, color: Color) {
        let end = (x + length).min(self.width);
        for px in x..end {
            self.draw_pixel(px, y, color);
        }
    }

    /// 绘制垂直线
    pub fn draw_vline(&mut self, x: usize, y: usize, length: usize, color: Color) {
        let end = (y + length).min(self.height);
        for py in y..end {
            self.draw_pixel(x, py, color);
        }
    }

    /// 绘制矩形边框
    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        if w == 0 || h == 0 {
            return;
        }
        self.draw_hline(x, y, w, color);
        self.draw_hline(x, y + h - 1, w, color);
        self.draw_vline(x, y, h, color);
        self.draw_vline(x + w - 1, y, h, color);
    }

    /// 使用 Blt 操作绘制矩形 (更高效)
    pub fn blt_fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let pixels: Vec<BltPixel> = alloc::vec![color.to_blt_pixel(); w * h];

        let _ = self.gop.blt(BltOp::BufferToVideo {
            buffer: &pixels,
            src: BltRegion::Full,
            dest: (x, y),
            dims: (w, h),
        });
    }

    /// 绘制字符 (简单位图字体)
    pub fn draw_char(&mut self, x: usize, y: usize, c: char, fg: Color, bg: Option<Color>) {
        for (row, &bits) in get_font_bitmap(c).iter().enumerate() {
            for col in 0..8 {
                let px = x + col;
                let py = y + row;

                if px >= self.width || py >= self.height {
                    continue;
                }

                let bit = (bits >> (7 - col)) & 1;
                if bit != 0 {
                    self.draw_pixel(px, py, fg);
                } else if let Some(bg_color) = bg {
                    self.draw_pixel(px, py, bg_color);
                }
            }
        }
    }

    /// 绘制字符串
    pub fn draw_string(
        &mut self,
        x: usize,
        y: usize,
        s: &str,
        fg: Color,
        bg: Option<Color>,
    ) -> usize {
        let mut cursor_x = x;
        let char_width = 8;
        for c in s.chars() {
            if cursor_x + char_width > self.width {
                break;
            }

            self.draw_char(cursor_x, y, c, fg, bg);
            cursor_x += char_width;
        }

        cursor_x
    }

    /// 计算字符串宽度 (像素)
    pub fn string_width(&self, s: &str) -> usize {
        s.len() * 8
    }

    /// 绘制居中字符串
    pub fn draw_string_centered(&mut self, y: usize, s: &str, fg: Color, bg: Option<Color>) {
        let text_width = self.string_width(s);
        let x = (self.width.saturating_sub(text_width)) / 2;
        self.draw_string(x, y, s, fg, bg);
    }
}
