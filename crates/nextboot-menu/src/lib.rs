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
use alloc::format;

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

impl MenuItem {
    /// 创建新的菜单项
    pub fn new(label: String, path: String, size: u64, iso_type: IsoType) -> Self {
        Self {
            label,
            path,
            size,
            iso_type,
        }
    }

    /// 从文件信息创建
    pub fn from_file_info(path: &str, size: u64) -> Self {
        let label = path.split('/')
            .last()
            .unwrap_or(path)
            .to_string();

        let iso_type = IsoType::detect_from_path(path);

        Self {
            label,
            path: path.to_string(),
            size,
            iso_type,
        }
    }
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

    /// 获取显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            IsoType::Windows => "Windows",
            IsoType::Ubuntu => "Ubuntu",
            IsoType::Debian => "Debian",
            IsoType::Fedora => "Fedora",
            IsoType::Arch => "Arch Linux",
            IsoType::GenericLinux => "Linux",
            IsoType::WinPE => "WinPE",
            IsoType::Unknown => "Unknown",
        }
    }

    /// 根据 ISO 内容检测类型
    pub fn detect(files: &[&str]) -> Self {
        // Windows 特征文件
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("bootmgfw.efi") || f_lower.contains("install.wim") || f_lower.contains("install.esd")
        }) {
            return IsoType::Windows;
        }

        // WinPE
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("boot.sdi") && f_lower.contains("winpe")
        }) {
            return IsoType::WinPE;
        }

        // Ubuntu
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("casper/vmlinuz") || f_lower.contains(".disk/info")
        }) {
            return IsoType::Ubuntu;
        }

        // Debian
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("install.amd") || f_lower.contains("install.386")
        }) {
            return IsoType::Debian;
        }

        // Fedora
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("images/pxeboot") || f_lower.contains("fedora")
        }) {
            return IsoType::Fedora;
        }

        // Arch
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("arch/boot")
        }) {
            return IsoType::Arch;
        }

        // 通用 Linux
        if files.iter().any(|f| {
            let f_lower = f.to_lowercase();
            f_lower.contains("vmlinuz") || f_lower.contains("initrd") || f_lower.contains("grub.cfg")
        }) {
            return IsoType::GenericLinux;
        }

        IsoType::Unknown
    }

    /// 从路径检测类型
    pub fn detect_from_path(path: &str) -> Self {
        let path_lower = path.to_lowercase();

        if path_lower.contains("windows") {
            return IsoType::Windows;
        }
        if path_lower.contains("ubuntu") {
            return IsoType::Ubuntu;
        }
        if path_lower.contains("debian") {
            return IsoType::Debian;
        }
        if path_lower.contains("fedora") {
            return IsoType::Fedora;
        }
        if path_lower.contains("arch") {
            return IsoType::Arch;
        }
        if path_lower.contains("winpe") || path_lower.contains("pe_") {
            return IsoType::WinPE;
        }
        if path_lower.contains("linux") {
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
    /// 过滤器
    pub filter: String,
}

impl MenuState {
    /// 创建新菜单
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            selected: 0,
            scroll_offset: 0,
            dirty: true,
            filter: String::new(),
        }
    }

    /// 创建空菜单
    pub fn empty() -> Self {
        Self::new(Vec::new())
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

    /// 移动到第一项
    pub fn move_first(&mut self) {
        if !self.items.is_empty() && self.selected != 0 {
            self.selected = 0;
            self.dirty = true;
        }
    }

    /// 移动到最后一项
    pub fn move_last(&mut self) {
        let last = self.items.len().saturating_sub(1);
        if self.selected != last {
            self.selected = last;
            self.dirty = true;
        }
    }

    /// 翻页
    pub fn page_up(&mut self, page_size: usize) {
        if page_size > 0 && self.selected > 0 {
            self.selected = self.selected.saturating_sub(page_size);
            self.dirty = true;
        }
    }

    /// 翻页
    pub fn page_down(&mut self, page_size: usize) {
        if page_size > 0 {
            let max = self.items.len().saturating_sub(1);
            self.selected = (self.selected + page_size).min(max);
            self.dirty = true;
        }
    }

    /// 获取当前选中项
    pub fn selected_item(&self) -> Option<&MenuItem> {
        self.items.get(self.selected)
    }

    /// 获取可显示范围
    pub fn visible_range(&self, max_items: usize) -> core::ops::Range<usize> {
        if self.items.is_empty() {
            return 0..0;
        }

        // 确保选中项可见
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_items {
            self.scroll_offset = self.selected - max_items + 1;
        }

        let end = (self.scroll_offset + max_items).min(self.items.len());
        self.scroll_offset..end
    }

    /// 设置过滤器
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// 获取过滤后的项目
    pub fn filtered_items(&self) -> Vec<&MenuItem> {
        if self.filter.is_empty() {
            self.items.iter().collect()
        } else {
            self.items.iter()
                .filter(|item| item.label.to_lowercase().contains(&self.filter))
                .collect()
        }
    }

    /// 项目数量
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 添加项目
    pub fn add(&mut self, item: MenuItem) {
        self.items.push(item);
        self.dirty = true;
    }

    /// 清空项目
    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// 排序项目 (按名称)
    pub fn sort_by_name(&mut self) {
        self.items.sort_by(|a, b| a.label.cmp(&b.label));
        self.dirty = true;
    }

    /// 排序项目 (按大小)
    pub fn sort_by_size(&mut self) {
        self.items.sort_by(|a, b| b.size.cmp(&a.size));
        self.dirty = true;
    }

    /// 排序项目 (按类型)
    pub fn sort_by_type(&mut self) {
        self.items.sort_by(|a, b| a.iso_type.display_name().cmp(b.iso_type.display_name()));
        self.dirty = true;
    }
}

/// 用户输入
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    Refresh,
    PageUp,
    PageDown,
    Home,
    End,
    Char(char),
    Other,
}

impl Input {
    /// 从 UEFI 按键创建
    pub fn from_uefi_key(scan_code: u16, char_code: Option<char>) -> Self {
        match scan_code {
            0x01 => Input::Up,
            0x02 => Input::Down,
            0x03 => Input::Right,
            0x04 => Input::Left,
            0x05 => Input::Home,
            0x06 => Input::End,
            0x07 => Input::Refresh, // Insert
            0x08 => Input::PageUp,  // Page Up
            0x09 => Input::PageDown, // Page Down
            0x17 => Input::Escape,
            _ => {
                if let Some(c) = char_code {
                    match c {
                        '\r' | '\n' => Input::Enter,
                        '\x1b' => Input::Escape,
                        '\t' => Input::Tab,
                        'w' | 'W' => Input::Up,
                        's' | 'S' => Input::Down,
                        'a' | 'A' => Input::Left,
                        'd' | 'D' => Input::Right,
                        'r' | 'R' => Input::Refresh,
                        c => Input::Char(c),
                    }
                } else {
                    Input::Other
                }
            }
        }
    }
}

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

/// 菜单主题
#[derive(Debug, Clone)]
pub struct MenuTheme {
    /// 背景色
    pub background: gop::Color,
    /// 前景色
    pub foreground: gop::Color,
    /// 选中背景色
    pub selection_bg: gop::Color,
    /// 选中前景色
    pub selection_fg: gop::Color,
    /// 标题颜色
    pub title_color: gop::Color,
    /// 边框颜色
    pub border_color: gop::Color,
    /// 帮助文本颜色
    pub help_color: gop::Color,
}

impl Default for MenuTheme {
    fn default() -> Self {
        Self {
            background: gop::Color::BLACK,
            foreground: gop::Color::WHITE,
            selection_bg: gop::Color::BLUE,
            selection_fg: gop::Color::WHITE,
            title_color: gop::Color::CYAN,
            border_color: gop::Color::GRAY,
            help_color: gop::Color::DARK_GRAY,
        }
    }
}

impl MenuTheme {
    /// 深色主题
    pub fn dark() -> Self {
        Self::default()
    }

    /// 浅色主题
    pub fn light() -> Self {
        Self {
            background: gop::Color::WHITE,
            foreground: gop::Color::BLACK,
            selection_bg: gop::Color::BLUE,
            selection_fg: gop::Color::WHITE,
            title_color: gop::Color::BLUE,
            border_color: gop::Color::GRAY,
            help_color: gop::Color::DARK_GRAY,
        }
    }

    /// Ubuntu 主题
    pub fn ubuntu() -> Self {
        Self {
            background: gop::Color { r: 44, g: 0, b: 61, a: 255 }, // Ubuntu purple
            foreground: gop::Color::WHITE,
            selection_bg: gop::Color { r: 233, g: 84, b: 32, a: 255 }, // Ubuntu orange
            selection_fg: gop::Color::WHITE,
            title_color: gop::Color { r: 233, g: 84, b: 32, a: 255 },
            border_color: gop::Color::GRAY,
            help_color: gop::Color { r: 128, g: 128, b: 128, a: 255 },
        }
    }
}

/// 菜单配置
#[derive(Debug, Clone)]
pub struct MenuConfig {
    /// 标题
    pub title: String,
    /// 主题
    pub theme: MenuTheme,
    /// 显示帮助
    pub show_help: bool,
    /// 显示文件大小
    pub show_size: bool,
    /// 显示类型图标
    pub show_type_icon: bool,
    /// 自动选择超时 (秒)
    pub timeout: Option<u64>,
    /// 默认选择索引
    pub default_selection: usize,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            title: String::from("NextBoot"),
            theme: MenuTheme::default(),
            show_help: true,
            show_size: true,
            show_type_icon: true,
            timeout: None,
            default_selection: 0,
        }
    }
}
