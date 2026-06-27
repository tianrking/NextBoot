use crate::gop::{Color, GopContext};
use crate::{format_size, MenuConfig, MenuState};
use alloc::format;

/// 图形菜单渲染器 (使用 GOP)
pub struct GraphicalMenuRenderer {
    gop: GopContext<'static>,
    config: MenuConfig,
    width: usize,
    height: usize,
}

impl GraphicalMenuRenderer {
    /// 创建图形菜单渲染器
    pub fn new(gop: GopContext<'static>, config: MenuConfig) -> Self {
        let (width, height) = gop.resolution();
        Self {
            gop,
            config,
            width,
            height,
        }
    }

    /// 渲染菜单
    pub fn render(&mut self, state: &mut MenuState) {
        let theme = &self.config.theme;

        self.gop.clear(theme.background);
        self.draw_title();
        self.draw_items(state);

        if self.config.show_help {
            self.draw_help();
        }
    }

    /// 绘制标题
    fn draw_title(&mut self) {
        let theme = &self.config.theme;
        let title = &self.config.title;
        let title_height = 60;

        self.gop
            .fill_rect(0, 0, self.width, title_height, theme.title_color);

        let y = (title_height - 16) / 2;
        self.gop
            .draw_string_centered(y, title, theme.background, None);
    }

    /// 绘制菜单项
    fn draw_items(&mut self, state: &mut MenuState) {
        let theme = &self.config.theme;
        let item_height = 24;
        let start_y = 80;
        let margin = 20;
        let max_items = (self.height - start_y - 60) / item_height;

        let visible = state.visible_range(max_items);

        for (i, idx) in visible.clone().enumerate() {
            let item = &state.items[idx];
            let is_selected = idx == state.selected;
            let y = start_y + i * item_height;

            let bg_color = if is_selected {
                theme.selection_bg
            } else {
                theme.background
            };
            self.gop.fill_rect(
                margin,
                y,
                self.width - margin * 2,
                item_height - 2,
                bg_color,
            );

            let fg_color = if is_selected {
                theme.selection_fg
            } else {
                theme.foreground
            };

            let icon = item.iso_type.icon();
            let icon_x = margin + 10;
            self.gop.draw_string(icon_x, y + 4, icon, fg_color, None);

            let name_x = icon_x + 50;
            let name_width = self.width - margin * 2 - 200;
            let name = if self.gop.string_width(&item.label) > name_width {
                let mut truncated = item.label.clone();
                while self.gop.string_width(&truncated) > name_width - 24 && !truncated.is_empty() {
                    truncated.pop();
                }
                format!("{}...", truncated)
            } else {
                item.label.clone()
            };
            self.gop.draw_string(name_x, y + 4, &name, fg_color, None);

            let size_str = format_size(item.size);
            let size_x = self.width - margin - self.gop.string_width(&size_str) - 10;
            self.gop
                .draw_string(size_x, y + 4, &size_str, theme.help_color, None);
        }
    }

    /// 绘制帮助
    fn draw_help(&mut self) {
        let theme = &self.config.theme;
        let y = self.height - 40;

        let help_text = "↑↓: Select | Enter: Boot | R: Refresh | Esc: Reboot";
        self.gop
            .draw_string_centered(y, help_text, theme.help_color, None);
    }

    /// 显示消息
    pub fn show_message(&mut self, msg: &str, is_error: bool) {
        let y = self.height / 2;
        let color = if is_error { Color::RED } else { Color::YELLOW };
        let padding = 20;
        let msg_width = self.gop.string_width(msg) + padding * 2;
        let x = (self.width - msg_width) / 2;

        self.gop.fill_rect(x, y - 10, msg_width, 36, color);
        self.gop
            .draw_string_centered(y, msg, self.config.theme.background, None);
    }
}
