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

use alloc::string::String;
use alloc::vec::Vec;

pub mod gop;
pub mod console;
pub mod menu;

/// 菜单项
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// 显示名称
    pub label: String,
    /// 文件路径
    pub path: String,
    /// 文件大小
    pub size: u64,
    /// 检测到的类型
    pub iso_type: IsoType,
}

/// ISO 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoType {
    /// Windows 安装镜像
    Windows,
    /// Ubuntu
    Ubuntu,
    /// Debian
    Debian,
    /// Fedora
    Fedora,
    /// Arch Linux
    Arch,
    /// 通用 Linux
    GenericLinux,
    /// PE 环境
    WinPE,
    /// 未知类型
    Unknown,
}

impl IsoType {
    /// 获取显示图标
    pub fn icon(&self) -> &'static str {
        match self {
            IsoType::Windows => "[W]",
            IsoType::Ubuntu => "[U]",
            IsoType::Debian => "[D]",
            IsoType::Fedora => "[F]",
            IsoType::Arch => "[A]",
            IsoType::GenericLinux => "[L]",
            IsoType::WinPE => "[P]",
            IsoType::Unknown => "[?]",
        }
    }

    /// 根据 ISO 内容检测类型
    pub fn detect(files: &[&str]) -> Self {
        // Windows 特征文件
        if files.iter().any(|f| f.contains("bootmgfw.efi") || f.contains("install.wim")) {
            return IsoType::Windows;
        }

        // Ubuntu
        if files.iter().any(|f| f.contains(".disk/info") || f.contains("casper/vmlinuz")) {
            return IsoType::Ubuntu;
        }

        // Debian
        if files.iter().any(|f| f.contains(".disk/info") && f.contains("install.amd")) {
            return IsoType::Debian;
        }

        // Fedora
        if files.iter().any(|f| f.contains("EFI/BOOT/BOOTX64.EFI") && f.contains("images/pxeboot")) {
            return IsoType::Fedora;
        }

        // Arch
        if files.iter().any(|f| f.contains("arch/boot")) {
            return IsoType::Arch;
        }

        // 通用 Linux
        if files.iter().any(|f| f.contains("vmlinuz") || f.contains("initrd")) {
            return IsoType::GenericLinux;
        }

        IsoType::Unknown
    }
}

/// 菜单状态
#[derive(Debug, Clone)]
pub struct MenuState {
    /// 所有菜单项
    pub items: Vec<MenuItem>,
    /// 当前选中索引
    pub selected: usize,
    /// 滚动偏移
    pub scroll_offset: usize,
    /// 是否需要重绘
    pub dirty: bool,
}

impl MenuState {
    /// 创建新菜单
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            selected: 0,
            scroll_offset: 0,
            dirty: true,
        }
    }

    /// 移动选择
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.dirty = true;
        }
    }

    /// 移动选择
    pub fn move_down(&mut self) {
        if self.selected < self.items.len().saturating_sub(1) {
            self.selected += 1;
            self.dirty = true;
        }
    }

    /// 获取当前选中项
    pub fn selected_item(&self) -> Option<&MenuItem> {
        self.items.get(self.selected)
    }

    /// 获取可显示范围
    pub fn visible_range(&self, max_items: usize) -> core::ops::Range<usize> {
        // 确保选中项可见
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_items {
            self.scroll_offset = self.selected - max_items + 1;
        }

        let end = (self.scroll_offset + max_items).min(self.items.len());
        self.scroll_offset..end
    }
}

/// 用户输入
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Up,
    Down,
    Enter,
    Escape,
    Refresh,
    Other,
}

/// 格式化文件大小
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        alloc::format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        alloc::format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        alloc::format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        alloc::format!("{} B", bytes)
    }
}
