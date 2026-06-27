//! 菜单渲染

use crate::console::{clear_rect, ConsoleContext};
use crate::gop::Color;
use crate::{format_size, Input, MenuConfig, MenuItem, MenuState};
use alloc::format;
use alloc::string::String;

pub use crate::graphical::GraphicalMenuRenderer;

/// 菜单渲染器
pub struct MenuRenderer<'a, 'c> {
    console: &'a mut ConsoleContext<'c>,
    config: MenuConfig,
    width: usize,
    height: usize,
}

impl<'a, 'c> MenuRenderer<'a, 'c> {
    /// 创建渲染器
    pub fn new(console: &'a mut ConsoleContext<'c>, config: MenuConfig) -> Self {
        let (width, height) = console.size();
        Self {
            console,
            config,
            width,
            height,
        }
    }

    /// 渲染完整菜单
    pub fn render(&mut self, state: &mut MenuState) {
        self.console.clear();

        // 绘制标题
        self.render_header();

        // 绘制菜单项
        self.render_items(state);

        // 绘制底部帮助
        if self.config.show_help {
            self.render_footer();
        }

        state.dirty = false;
    }

    /// 仅更新选中状态 (更高效)
    pub fn update_selection(&mut self, state: &mut MenuState, prev_selected: usize) {
        let start_row = 4;
        let max_items = self.height - start_row - 3;

        let visible = state.visible_range(max_items);

        // 更新之前选中的行
        if visible.contains(&prev_selected) {
            let row = start_row + prev_selected - visible.start;
            self.render_item(state, prev_selected, row, false);
        }

        // 更新当前选中的行
        if visible.contains(&state.selected) {
            let row = start_row + state.selected - visible.start;
            self.render_item(state, state.selected, row, true);
        }
    }

    /// 渲染标题
    fn render_header(&mut self) {
        let theme = &self.config.theme;

        self.console.set_cursor(0, 0);
        self.console.set_color(theme.title_color, theme.background);
        self.console
            .println(&format!("╔{}╗", "═".repeat(self.width - 2)));

        // 居中标题
        let title = &self.config.title;
        let title_padding = (self.width - title.len() - 4) / 2;
        self.console.set_color(theme.title_color, theme.background);
        self.console.println(&format!(
            "║{}{}{}║",
            " ".repeat(title_padding),
            title,
            " ".repeat(self.width - title_padding - title.len() - 4)
        ));

        self.console.set_color(theme.border_color, theme.background);
        self.console
            .println(&format!("╠{}╣", "═".repeat(self.width - 2)));
    }

    /// 渲染菜单项
    fn render_items(&mut self, state: &mut MenuState) {
        let start_row = 4;
        let max_items = self.height - start_row - 3;
        let theme = &self.config.theme;

        let visible = state.visible_range(max_items);

        // 清除菜单区域
        for row in start_row..start_row + max_items {
            self.console.set_cursor(0, row);
            self.console.set_color(theme.background, theme.foreground);
            self.console.print(&" ".repeat(self.width));
        }

        // 渲染可见项目
        for (i, idx) in visible.clone().enumerate() {
            let is_selected = idx == state.selected;
            let row = start_row + i;
            self.render_item(state, idx, row, is_selected);
        }

        // 显示滚动指示
        if state.items.len() > max_items {
            self.render_scroll_indicator(state, start_row, max_items);
        }
    }

    /// 渲染单个菜单项
    fn render_item(&mut self, state: &MenuState, idx: usize, row: usize, is_selected: bool) {
        let item = &state.items[idx];
        let theme = &self.config.theme;

        self.console.set_cursor(1, row);

        // 设置颜色
        if is_selected {
            self.console
                .set_color(theme.selection_fg, theme.selection_bg);
        } else {
            self.console.set_color(theme.foreground, theme.background);
        }

        // 格式化行
        let icon = if self.config.show_type_icon {
            item.iso_type.icon()
        } else {
            ""
        };

        let size_str = if self.config.show_size {
            format_size(item.size)
        } else {
            String::new()
        };

        // 计算名称可用宽度
        let used_width = 4 + icon.len() + 1 + size_str.len() + 1;
        let name_width = self.width.saturating_sub(used_width);

        // 截断或填充名称
        let name = if item.label.len() > name_width {
            format!("{}...", &item.label[..name_width.saturating_sub(3)])
        } else {
            format!("{:width$}", item.label, width = name_width)
        };

        let line = format!(
            " {} {} {:<width$} {} ",
            if is_selected { ">" } else { " " },
            icon,
            name,
            size_str,
            width = name_width
        );

        self.console.print(&line);
    }

    /// 渲染滚动指示器
    fn render_scroll_indicator(&mut self, state: &MenuState, start_row: usize, max_items: usize) {
        let theme = &self.config.theme;
        let total = state.items.len();
        let start = state.scroll_offset;
        let end = (start + max_items).min(total);

        // 右侧滚动条
        let scrollbar_col = self.width - 1;

        for i in 0..max_items {
            let row = start_row + i;
            self.console.set_cursor(scrollbar_col, row);
            self.console.set_color(theme.border_color, theme.background);

            let pos = (i * total) / max_items;
            if pos >= start && pos < end {
                self.console.print("█");
            } else {
                self.console.print("░");
            }
        }
    }

    /// 渲染底部帮助
    fn render_footer(&mut self) {
        let theme = &self.config.theme;
        let footer_row = self.height - 2;

        self.console.set_cursor(0, footer_row);
        self.console.set_color(theme.border_color, theme.background);
        self.console
            .println(&format!("╠{}╣", "═".repeat(self.width - 2)));

        self.console.set_cursor(0, footer_row + 1);
        self.console.set_color(theme.help_color, theme.background);

        let help_text = " ↑↓: 选择 | Enter: 启动 | R: 刷新 | Esc: 重启 ";
        let padding = (self.width - help_text.len() - 2) / 2;
        self.console.println(&format!(
            "║{}{}{}║",
            " ".repeat(padding),
            help_text,
            " ".repeat(self.width - padding - help_text.len() - 4)
        ));
    }

    /// 显示消息
    pub fn show_message(&mut self, msg: &str, is_error: bool) {
        let row = self.height / 2;
        let col = (self.width - msg.len()) / 2;

        let theme = &self.config.theme;
        if is_error {
            self.console.set_color(Color::RED, theme.background);
        } else {
            self.console.set_color(Color::YELLOW, theme.background);
        }

        self.console.set_cursor(col, row);
        self.console.print(msg);
    }

    /// 显示加载提示
    pub fn show_loading(&mut self, msg: &str) {
        let row = self.height / 2;
        let col = (self.width - msg.len() - 3) / 2;

        self.console.set_cursor(col, row);
        self.console.print(msg);
        self.console.print("...");
    }

    /// 清除消息
    pub fn clear_message(&mut self) {
        let row = self.height / 2;
        clear_rect(self.console, 0, row, self.width, 1);
    }

    /// 显示进度
    pub fn show_progress(&mut self, current: usize, total: usize, msg: &str) {
        let row = self.height / 2;
        let progress = if total > 0 {
            current as f32 / total as f32
        } else {
            0.0
        };

        crate::console::show_progress(
            self.console,
            10,
            row,
            self.width - 20,
            progress,
            self.config.theme.selection_bg,
            self.config.theme.background,
        );

        self.console.set_cursor(10, row + 1);
        self.console.print(msg);
    }

    /// 显示倒计时
    pub fn show_countdown(&mut self, seconds: u64, action: &str) {
        let row = self.height - 4;
        let msg = format!(
            "{} in {} seconds... (Press any key to cancel)",
            action, seconds
        );
        let col = (self.width - msg.len()) / 2;

        self.console.set_cursor(col, row);
        self.console
            .set_color(self.config.theme.help_color, self.config.theme.background);
        self.console.print(&msg);
    }

    /// 清除倒计时
    pub fn clear_countdown(&mut self) {
        let row = self.height - 4;
        clear_rect(self.console, 0, row, self.width, 1);
    }
}

/// 运行菜单交互循环
pub fn run_menu_loop<'a>(
    console: &mut ConsoleContext<'_>,
    bt: &uefi::table::boot::BootServices,
    state: &'a mut MenuState,
    config: MenuConfig,
) -> Option<&'a MenuItem> {
    {
        let mut renderer = MenuRenderer::new(console, config.clone());
        renderer.render(state);
    }

    loop {
        // 等待输入
        let input = console.wait_for_key(bt);

        match input {
            Input::Up => {
                let prev = state.selected;
                state.move_up();
                if state.dirty {
                    let mut renderer = MenuRenderer::new(console, config.clone());
                    renderer.update_selection(state, prev);
                }
            }
            Input::Down => {
                let prev = state.selected;
                state.move_down();
                if state.dirty {
                    let mut renderer = MenuRenderer::new(console, config.clone());
                    renderer.update_selection(state, prev);
                }
            }
            Input::PageUp => {
                state.page_up(10);
                if state.dirty {
                    let mut renderer = MenuRenderer::new(console, config.clone());
                    renderer.render(state);
                }
            }
            Input::PageDown => {
                state.page_down(10);
                if state.dirty {
                    let mut renderer = MenuRenderer::new(console, config.clone());
                    renderer.render(state);
                }
            }
            Input::Home => {
                state.move_first();
                let mut renderer = MenuRenderer::new(console, config.clone());
                renderer.render(state);
            }
            Input::End => {
                state.move_last();
                let mut renderer = MenuRenderer::new(console, config.clone());
                renderer.render(state);
            }
            Input::Enter => {
                return state.selected_item();
            }
            Input::Escape => {
                return None;
            }
            Input::Refresh => {
                // 返回并请求刷新
                return None;
            }
            _ => {}
        }
    }
}
