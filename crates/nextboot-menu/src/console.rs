//! 控制台输出封装

use uefi::proto::console::text::{Input, Output, ScanCode};
use uefi::table::boot::BootServices;
use crate::{Input as MenuInput, Color};

/// 控制台上下文
pub struct ConsoleContext<'a> {
    stdout: &'a mut Output<'a>,
    stdin: &'a mut Input<'a>,
}

impl<'a> ConsoleContext<'a> {
    /// 创建控制台上下文
    pub fn new(stdout: &'a mut Output<'a>, stdin: &'a mut Input<'a>) -> Self {
        Self { stdout, stdin }
    }

    /// 清屏
    pub fn clear(&mut self) {
        self.stdout.reset(false).unwrap();
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

    /// 设置光标位置
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        let _ = self.stdout.set_cursor_position(col, row);
    }

    /// 设置颜色属性
    pub fn set_color(&mut self, fg: Color, bg: Color) {
        // UEFI 控制台颜色映射
        let fg_attr = Self::color_to_attr(fg);
        let bg_attr = Self::color_to_attr(bg) << 4;
        let _ = self.stdout.set_attribute(fg_attr | bg_attr);
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
    pub fn wait_for_key(&mut self) -> MenuInput {
        // 等待按键事件
        // 注意: 实际实现需要使用 BootServices.wait_for_event

        MenuInput::Other
    }

    /// 读取当前按键 (非阻塞)
    pub fn read_key(&mut self) -> Option<MenuInput> {
        if let Ok(Some(key)) = self.stdin.read_key() {
            match key {
                uefi::proto::console::text::Key::Special(sc) => {
                    match sc {
                        ScanCode::UP => Some(MenuInput::Up),
                        ScanCode::DOWN => Some(MenuInput::Down),
                        ScanCode::ESCAPE => Some(MenuInput::Escape),
                        _ => Some(MenuInput::Other),
                    }
                }
                uefi::proto::console::text::Key::Char(ch) => {
                    match ch {
                        'w' | 'W' => Some(MenuInput::Up),
                        's' | 'S' => Some(MenuInput::Down),
                        '\r' | '\n' => Some(MenuInput::Enter),
                        'r' | 'R' => Some(MenuInput::Refresh),
                        _ => Some(MenuInput::Other),
                    }
                }
            }
        } else {
            None
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
