//! 控制台输出封装

use uefi::proto::console::text::{Input as UefiInput, Output, ScanCode};
use uefi::table::boot::BootServices;
use crate::Input as MenuInput;
use crate::gop::Color;
use alloc::vec::Vec;
use alloc::string::ToString;

/// 控制台上下文
pub struct ConsoleContext<'a> {
    stdout: &'a mut Output,
    stdin: &'a mut UefiInput,
    width: usize,
    height: usize,
}

impl<'a> ConsoleContext<'a> {
    /// 创建控制台上下文
    pub fn new(stdout: &'a mut Output, stdin: &'a mut UefiInput) -> Self {
        let (width, height) = get_console_size(stdout);
        Self { stdout, stdin, width, height }
    }

    /// 获取控制台尺寸
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// 清屏
    pub fn clear(&mut self) {
        let _ = self.stdout.reset(false);
    }

    /// 打印字符串
    pub fn print(&mut self, s: &str) {
        let _ = self.stdout.output_string(s);
    }

    /// 打印行
    pub fn println(&mut self, s: &str) {
        self.print(s);
        self.print("\r\n");
    }

    /// 打印格式化字符串
    pub fn print_fmt(&mut self, args: core::fmt::Arguments<'_>) {
        use core::fmt::Write;
        let _ = self.stdout.write_fmt(args);
    }

    /// 设置光标位置
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        let _ = self.stdout.set_cursor_position(col, row);
    }

    /// 隐藏光标
    pub fn hide_cursor(&mut self) {
        let _ = self.stdout.enable_cursor(false);
    }

    /// 显示光标
    pub fn show_cursor(&mut self) {
        let _ = self.stdout.enable_cursor(true);
    }

    /// 设置颜色属性
    pub fn set_color(&mut self, fg: Color, bg: Color) {
        // UEFI 控制台颜色映射
        let fg_attr = Self::color_to_attr(fg);
        let bg_attr = Self::color_to_attr(bg) << 4;
        let _ = self.stdout.set_attribute(fg_attr | bg_attr);
    }

    /// 设置前景色
    pub fn set_fg(&mut self, color: Color) {
        let attr = Self::color_to_attr(color);
        let _ = self.stdout.set_attribute(attr);
    }

    /// 颜色转控制台属性
    fn color_to_attr(color: Color) -> usize {
        match (color.r > 128, color.g > 128, color.b > 128) {
            (false, false, false) => 0, // Black
            (false, false, true) => 1,  // Blue
            (false, true, false) => 2,  // Green
            (false, true, true) => 3,   // Cyan
            (true, false, false) => 4,  // Red
            (true, false, true) => 5,   // Magenta
            (true, true, false) => 6,   // Brown
            (true, true, true) => 7,    // Light Gray
        }
    }

    /// 等待并读取输入
    pub fn wait_for_key(&mut self, bt: &BootServices) -> MenuInput {
        // 等待按键事件
        let events = [self.stdin.wait_for_key_event()];
        let _ = bt.wait_for_event(&events);

        self.read_key().unwrap_or(MenuInput::Other)
    }

    /// 读取当前按键 (非阻塞)
    pub fn read_key(&mut self) -> Option<MenuInput> {
        if let Ok(Some(key)) = self.stdin.read_key() {
            match key {
                uefi::proto::console::text::Key::Special(sc) => {
                    Some(MenuInput::from_uefi_key(sc as u16, None))
                }
                uefi::proto::console::text::Key::Char(ch) => {
                    Some(MenuInput::from_uefi_key(0, Some(ch)))
                }
            }
        } else {
            None
        }
    }

    /// 读取原始按键
    pub fn read_raw_key(&mut self) -> Option<uefi::proto::console::text::Key> {
        self.stdin.read_key().ok().flatten()
    }

    /// 检查是否有按键
    pub fn has_key(&mut self) -> bool {
        self.stdin.read_key().ok().flatten().is_some()
    }

    /// 绘制水平线
    pub fn draw_hline(&mut self, row: usize, start_col: usize, length: usize, ch: char) {
        self.set_cursor(start_col, row);
        for _ in 0..length {
            self.print(&alloc::format!("{}", ch));
        }
    }

    /// 绘制垂直线
    pub fn draw_vline(&mut self, col: usize, start_row: usize, length: usize, ch: char) {
        for row in start_row..start_row + length {
            self.set_cursor(col, row);
            self.print(&alloc::format!("{}", ch));
        }
    }
}

/// 获取控制台尺寸
pub fn get_console_size(stdout: &Output) -> (usize, usize) {
    match stdout.query_mode(None) {
        Ok((cols, rows)) => (cols, rows),
        Err(_) => (80, 25), // 默认值
    }
}

/// 绘制边框
pub fn draw_border(console: &mut ConsoleContext, x: usize, y: usize, w: usize, h: usize) {
    // 顶部
    console.set_cursor(x, y);
    console.print("┌");
    for _ in 1..w - 1 {
        console.print("─");
    }
    console.print("┐");

    // 侧边
    for row in 1..h - 1 {
        console.set_cursor(x, y + row);
        console.print("│");
        console.set_cursor(x + w - 1, y + row);
        console.print("│");
    }

    // 底部
    console.set_cursor(x, y + h - 1);
    console.print("└");
    for _ in 1..w - 1 {
        console.print("─");
    }
    console.print("┘");
}

/// 绘制双线边框
pub fn draw_double_border(console: &mut ConsoleContext, x: usize, y: usize, w: usize, h: usize) {
    // 顶部
    console.set_cursor(x, y);
    console.print("╔");
    for _ in 1..w - 1 {
        console.print("═");
    }
    console.print("╗");

    // 侧边
    for row in 1..h - 1 {
        console.set_cursor(x, y + row);
        console.print("║");
        console.set_cursor(x + w - 1, y + row);
        console.print("║");
    }

    // 底部
    console.set_cursor(x, y + h - 1);
    console.print("╚");
    for _ in 1..w - 1 {
        console.print("═");
    }
    console.print("╝");
}

/// 清除矩形区域
pub fn clear_rect(console: &mut ConsoleContext, x: usize, y: usize, w: usize, h: usize) {
    let empty_line = " ".repeat(w);
    for row in y..y + h {
        console.set_cursor(x, row);
        console.print(&empty_line);
    }
}

/// 绘制文本框
pub fn draw_text_box(
    console: &mut ConsoleContext,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    title: Option<&str>,
) {
    draw_border(console, x, y, w, h);

    if let Some(title_text) = title {
        // 绘制标题
        let title_x = x + 2;
        let title_y = y;
        console.set_cursor(title_x, title_y);
        console.print("┤");
        console.print(title_text);
        console.print("├");
    }
}

/// 显示进度条
pub fn show_progress(
    console: &mut ConsoleContext,
    x: usize,
    y: usize,
    width: usize,
    progress: f32, // 0.0 - 1.0
    fg: Color,
    bg: Color,
) {
    let filled = ((width - 2) as f32 * progress) as usize;

    console.set_cursor(x, y);
    console.print("[");

    // 保存当前颜色
    console.set_color(fg, bg);

    for i in 0..width - 2 {
        if i < filled {
            console.print("█");
        } else {
            console.print("░");
        }
    }

    // 恢复颜色
    console.set_color(Color::WHITE, Color::BLACK);
    console.print("]");
}

/// 显示消息框
pub fn show_message_box(
    console: &mut ConsoleContext,
    title: &str,
    message: &str,
    width: usize,
) {
    let height = 5;
    let (con_w, con_h) = console.size();
    let x = (con_w.saturating_sub(width)) / 2;
    let y = (con_h.saturating_sub(height)) / 2;

    // 绘制边框
    draw_text_box(console, x, y, width, height, Some(title));

    // 显示消息
    let msg_lines: Vec<&str> = message.lines().collect();
    for (i, line) in msg_lines.iter().enumerate().take(height - 3) {
        console.set_cursor(x + 2, y + 1 + i);
        console.print(line);
    }

    // 显示确认提示
    console.set_cursor(x + 2, y + height - 2);
    console.print("Press any key to continue...");
}

/// 读取密码 (隐藏输入)
pub fn read_password(console: &mut ConsoleContext, prompt: &str) -> alloc::string::String {
    console.print(prompt);

    let mut password = alloc::string::String::new();

    loop {
        if let Some(key) = console.read_raw_key() {
            match key {
                uefi::proto::console::text::Key::Char('\r') |
                uefi::proto::console::text::Key::Char('\n') => {
                    console.println("");
                    break;
                }
                uefi::proto::console::text::Key::Char(c) if c == '\x08' || c == '\x7f' => {
                    if !password.is_empty() {
                        password.pop();
                        console.print("\x08 \x08"); // Backspace, space, backspace
                    }
                }
                uefi::proto::console::text::Key::Char(c) if c >= ' ' && c <= '~' => {
                    password.push(c);
                    console.print("*");
                }
                _ => {}
            }
        }
    }

    password
}

/// 读取一行输入
pub fn read_line(console: &mut ConsoleContext, prompt: &str) -> alloc::string::String {
    console.print(prompt);

    let mut line = alloc::string::String::new();

    loop {
        if let Some(key) = console.read_raw_key() {
            match key {
                uefi::proto::console::text::Key::Char('\r') |
                uefi::proto::console::text::Key::Char('\n') => {
                    console.println("");
                    break;
                }
                uefi::proto::console::text::Key::Char(c) if c == '\x08' || c == '\x7f' => {
                    if !line.is_empty() {
                        line.pop();
                        console.print("\x08 \x08");
                    }
                }
                uefi::proto::console::text::Key::Char(c) if c >= ' ' && c <= '~' => {
                    line.push(c);
                    console.print(&alloc::format!("{}", c));
                }
                _ => {}
            }
        }
    }

    line
}
