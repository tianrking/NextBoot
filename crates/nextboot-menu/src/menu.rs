//! 菜单渲染

use crate::{MenuItem, MenuState, IsoType, format_size};
use crate::console::{ConsoleContext, draw_border};

/// 菜单渲染器
pub struct MenuRenderer<'a> {
    console: &'a mut ConsoleContext<'a>,
    title: &'a str,
    width: usize,
    height: usize,
}

impl<'a> MenuRenderer<'a> {
    /// 创建渲染器
    pub fn new(console: &'a mut ConsoleContext<'a>, title: &'a str) -> Self {
        let (width, height) = crate::console::get_console_size(console.stdout);
        Self { console, title, width, height }
    }

    /// 渲染完整菜单
    pub fn render(&mut self, state: &MenuState) {
        self.console.clear();

        // 绘制标题
        self.render_header();

        // 绘制菜单项
        self.render_items(state);

        // 绘制底部帮助
        self.render_footer();

        state.dirty = false;
    }

    /// 渲染标题
    fn render_header(&mut self) {
        self.console.set_cursor(0, 0);
        self.console.println(&alloc::format!(
            "╔{}╗",
            "═".repeat(self.width - 2)
        ));

        // 居中标题
        let title_padding = (self.width - self.title.len() - 4) / 2;
        self.console.println(&alloc::format!(
            "║{}{}{}║",
            " ".repeat(title_padding),
            self.title,
            " ".repeat(self.width - title_padding - self.title.len() - 4)
        ));

        self.console.println(&alloc::format!(
            "╠{}╣",
            "═".repeat(self.width - 2)
        ));
    }

    /// 渲染菜单项
    fn render_items(&mut self, state: &MenuState) {
        let start_row = 4;
        let max_items = self.height - start_row - 3;

        let visible_range = state.visible_range(max_items);

        for (i, idx) in visible_range.enumerate() {
            let item = &state.items[idx];
            let is_selected = idx == state.selected;

            self.console.set_cursor(1, start_row + i);

            // 选中高亮
            if is_selected {
                self.console.set_color(
                    crate::gop::Color::BLACK,
                    crate::gop::Color::WHITE
                );
            } else {
                self.console.set_color(
                    crate::gop::Color::WHITE,
                    crate::gop::Color::BLACK
                );
            }

            // 格式化行
            let icon = item.iso_type.icon();
            let size_str = format_size(item.size);
            let name = if item.label.len() > self.width - 20 {
                &item.label[..self.width - 23]
            } else {
                &item.label
            };

            let line = alloc::format!(
                " {} {} {:<width$} {:>10} ",
                if is_selected { ">" } else { " " },
                icon,
                name,
                size_str,
                width = self.width - 20
            );

            self.console.print(&line);
        }
    }

    /// 渲染底部帮助
    fn render_footer(&mut self) {
        let footer_row = self.height - 2;

        self.console.set_cursor(0, footer_row);
        self.console.println(&alloc::format!(
            "╠{}╣",
            "═".repeat(self.width - 2)
        ));

        self.console.set_cursor(0, footer_row + 1);
        self.console.println(&alloc::format!(
            "║ ↑↓: 选择 | Enter: 启动 | R: 刷新 | Esc: 重启{}║",
            " ".repeat(self.width - 42)
        ));
    }

    /// 显示消息
    pub fn show_message(&mut self, msg: &str, is_error: bool) {
        let row = self.height / 2;
        let col = (self.width - msg.len()) / 2;

        if is_error {
            self.console.set_color(
                crate::gop::Color::RED,
                crate::gop::Color::BLACK
            );
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
}
