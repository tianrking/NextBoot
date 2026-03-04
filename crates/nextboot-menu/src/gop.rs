//! UEFI GOP (Graphics Output Protocol) 封装

use uefi::proto::console::gop::{GraphicsOutput, PixelFormat, ModeInfo};
use uefi::table::boot::BootServices;

/// 颜色 (RGBA)
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Self = Self { r: 255, g: 255, b: 255, a: 255 };
    pub const BLUE: Self = Self { r: 0, g: 0, b: 255, a: 255 };
    pub const GREEN: Self = Self { r: 0, g: 255, b: 0, a: 255 };
    pub const RED: Self = Self { r: 255, g: 0, b: 0, a: 255 };
    pub const GRAY: Self = Self { r: 128, g: 128, b: 128, a: 255 };
    pub const DARK_GRAY: Self = Self { r: 64, g: 64, b: 64, a: 255 };

    /// 转换为 U32 (根据像素格式)
    pub fn to_u32(&self, format: PixelFormat) -> u32 {
        match format {
            PixelFormat::RGB => {
                ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
            }
            PixelFormat::BGR => {
                ((self.b as u32) << 16) | ((self.g as u32) << 8) | (self.r as u32)
            }
            PixelFormat::Bitmask | PixelFormat::BltOnly => {
                // 简化处理，使用 RGB
                ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
            }
        }
    }
}

/// GOP 上下文
pub struct GopContext<'a> {
    gop: &'a mut GraphicsOutput<'a>,
    width: usize,
    height: usize,
    format: PixelFormat,
}

impl<'a> GopContext<'a> {
    /// 初始化 GOP
    pub fn init(bt: &BootServices) -> uefi::Result<Self> {
        let gop_handle = bt.find_handles::<GraphicsOutput>()?.first()
            .ok_or(uefi::Status::UNSUPPORTED)?
            .clone();

        let mut gop = bt.open_protocol::<GraphicsOutput>(
            gop_handle,
            uefi::table::boot::OpenProtocolAttributes::Exclusive,
        )?;

        // 尝试设置最佳分辨率
        let (width, height) = Self::find_best_mode(&gop);

        let format = gop.current_mode_info().pixel_format();

        Ok(Self {
            gop: &mut gop,
            width,
            height,
            format,
        })
    }

    /// 查找最佳分辨率
    fn find_best_mode(gop: &GraphicsOutput) -> (usize, usize) {
        // 默认使用当前模式
        let info = gop.current_mode_info();
        (info.resolution().0, info.resolution().1)
    }

    /// 获取帧缓冲区
    pub fn frame_buffer(&mut self) -> &mut [u8] {
        self.gop.frame_buffer().as_mut_ptr()
    }

    /// 绘制像素
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        let fb = self.gop.frame_buffer().as_mut_ptr();
        let pixel_size = 4; // 32-bit pixels
        let offset = (y * self.width + x) * pixel_size;

        if offset + pixel_size <= fb.len() {
            let pixel = color.to_u32(self.format);
            let bytes = pixel.to_le_bytes();
            fb[offset..offset + 4].copy_from_slice(&bytes);
        }
    }

    /// 填充矩形
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        for py in y..(y + h).min(self.height) {
            for px in x..(x + w).min(self.width) {
                self.draw_pixel(px, py, color);
            }
        }
    }

    /// 清屏
    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// 获取分辨率
    pub fn resolution(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}
