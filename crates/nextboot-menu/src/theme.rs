use crate::gop;
use alloc::string::String;

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
            background: gop::Color {
                r: 44,
                g: 0,
                b: 61,
                a: 255,
            },
            foreground: gop::Color::WHITE,
            selection_bg: gop::Color {
                r: 233,
                g: 84,
                b: 32,
                a: 255,
            },
            selection_fg: gop::Color::WHITE,
            title_color: gop::Color {
                r: 233,
                g: 84,
                b: 32,
                a: 255,
            },
            border_color: gop::Color::GRAY,
            help_color: gop::Color {
                r: 128,
                g: 128,
                b: 128,
                a: 255,
            },
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
