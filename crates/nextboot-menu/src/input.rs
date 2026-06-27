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
            0x07 => Input::Refresh,
            0x08 => Input::PageUp,
            0x09 => Input::PageDown,
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
