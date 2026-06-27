//! UEFI GOP 菜单模块
//!
//! 提供基于 UEFI GOP 的图形/文本界面
//!
//! # 功能
//! - ISO 文件列表显示
//! - 键盘导航
//! - 状态提示

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;

pub mod console;
mod font;
pub mod gop;
mod graphical;
mod input;
mod item;
pub mod menu;
mod state;
mod theme;

pub use input::Input;
pub use item::{IsoType, MenuItem};
pub use state::MenuState;
pub use theme::{MenuConfig, MenuTheme};

/// 格式化文件大小
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 格式化时间
pub fn format_time(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}
