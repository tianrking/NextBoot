use super::{
    clean_plugin_path, JsonParser, VentoyConfigError, VentoyPassword, VentoyPathTarget,
    VentoyTargetKind,
};
use alloc::string::String;
use alloc::vec::Vec;

impl<'a> JsonParser<'a> {
    pub(super) fn parse_object_fields<F>(&mut self, mut field: F) -> Result<(), VentoyConfigError>
    where
        F: FnMut(&mut Self, &str) -> Result<(), VentoyConfigError>,
    {
        self.skip_ws();
        self.expect(b'{')?;
        loop {
            self.skip_ws();
            if self.consume(b'}') {
                break;
            }

            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            field(self, key.as_str())?;
            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b'}')?;
            break;
        }

        Ok(())
    }

    pub(super) fn parse_target(
        &mut self,
        kind: VentoyTargetKind,
    ) -> Result<Option<VentoyPathTarget>, VentoyConfigError> {
        let Some(path) = self.parse_optional_string()? else {
            self.skip_value()?;
            return Ok(None);
        };
        let Some(path) = clean_plugin_path(path) else {
            return Ok(None);
        };
        match kind {
            VentoyTargetKind::Image => Ok(Some(VentoyPathTarget::Image(path))),
            VentoyTargetKind::Parent => Ok(Some(VentoyPathTarget::Parent(path))),
        }
    }

    pub(super) fn parse_path_list_value(
        &mut self,
    ) -> Result<Option<Vec<String>>, VentoyConfigError> {
        self.skip_ws();
        if self.peek() == Some(b'"') {
            let value = self.parse_string()?;
            let mut values = Vec::new();
            values
                .try_reserve_exact(1)
                .map_err(|_| VentoyConfigError::OutOfMemory)?;
            values.push(value);
            return Ok(Some(values));
        }
        if self.peek() == Some(b'[') {
            return self.parse_string_array().map(Some);
        }

        self.skip_value()?;
        Ok(None)
    }

    pub(super) fn parse_optional_usize(&mut self) -> Result<Option<usize>, VentoyConfigError> {
        match self.parse_optional_i32()? {
            Some(value) if value >= 0 => Ok(Some(value as usize)),
            _ => Ok(None),
        }
    }

    pub(super) fn parse_optional_u32(&mut self) -> Result<Option<u32>, VentoyConfigError> {
        match self.parse_optional_i32()? {
            Some(value) if value >= 0 => Ok(Some(value as u32)),
            _ => Ok(None),
        }
    }

    pub(super) fn parse_optional_password(
        &mut self,
    ) -> Result<Option<VentoyPassword>, VentoyConfigError> {
        match self.parse_optional_string()? {
            Some(value) => Ok(VentoyPassword::parse(value.as_str())),
            None => {
                self.skip_value()?;
                Ok(None)
            }
        }
    }

    pub(super) fn parse_optional_config_path(
        &mut self,
    ) -> Result<Option<String>, VentoyConfigError> {
        match self.parse_optional_string()? {
            Some(value) => Ok(clean_plugin_path(value)),
            None => {
                self.skip_value()?;
                Ok(None)
            }
        }
    }

    pub(super) fn parse_optional_i32(&mut self) -> Result<Option<i32>, VentoyConfigError> {
        self.skip_ws();
        let start = self.pos;
        let negative = self.consume(b'-');
        let digits_start = self.pos;
        let mut value = 0i32;
        while let Some(byte @ b'0'..=b'9') = self.peek() {
            self.pos += 1;
            value = value
                .checked_mul(10)
                .and_then(|base| base.checked_add(i32::from(byte - b'0')))
                .ok_or(VentoyConfigError::InvalidJson)?;
        }

        if self.pos == digits_start {
            self.pos = start;
            self.skip_value()?;
            return Ok(None);
        }

        if negative {
            value = value.checked_neg().ok_or(VentoyConfigError::InvalidJson)?;
        }
        Ok(Some(value))
    }

    pub(super) fn parse_string_array(&mut self) -> Result<Vec<String>, VentoyConfigError> {
        let mut values = Vec::new();
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let value = self.parse_string()?;
            values
                .try_reserve_exact(1)
                .map_err(|_| VentoyConfigError::OutOfMemory)?;
            values.push(value);
            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }
        Ok(values)
    }

    pub(super) fn parse_optional_string(&mut self) -> Result<Option<String>, VentoyConfigError> {
        self.skip_ws();
        if self.peek() == Some(b'"') {
            return self.parse_string().map(Some);
        }
        Ok(None)
    }

    pub(super) fn skip_value(&mut self) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        match self.peek() {
            Some(b'"') => {
                let _ = self.parse_string()?;
            }
            Some(b'{') => {
                self.expect(b'{')?;
                loop {
                    self.skip_ws();
                    if self.consume(b'}') {
                        break;
                    }
                    let _ = self.parse_string()?;
                    self.skip_ws();
                    self.expect(b':')?;
                    self.skip_value()?;
                    self.skip_ws();
                    if self.consume(b',') {
                        continue;
                    }
                    self.expect(b'}')?;
                    break;
                }
            }
            Some(b'[') => {
                self.expect(b'[')?;
                loop {
                    self.skip_ws();
                    if self.consume(b']') {
                        break;
                    }
                    self.skip_value()?;
                    self.skip_ws();
                    if self.consume(b',') {
                        continue;
                    }
                    self.expect(b']')?;
                    break;
                }
            }
            Some(_) => {
                while let Some(ch) = self.peek() {
                    if matches!(ch, b',' | b'}' | b']') || ch.is_ascii_whitespace() {
                        break;
                    }
                    self.pos += 1;
                }
            }
            None => return Err(VentoyConfigError::InvalidJson),
        }
        Ok(())
    }

    pub(super) fn parse_string(&mut self) -> Result<String, VentoyConfigError> {
        self.skip_ws();
        self.expect(b'"')?;
        let mut out = String::new();
        while let Some(ch) = self.next() {
            match ch {
                b'"' => return Ok(out),
                b'\\' => {
                    let escaped = self.next().ok_or(VentoyConfigError::InvalidJson)?;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.parse_unicode_escape()?),
                        _ => return Err(VentoyConfigError::InvalidJson),
                    }
                }
                byte if byte < 0x20 => return Err(VentoyConfigError::InvalidJson),
                byte if byte.is_ascii() => out.push(byte as char),
                _ => out.push(self.consume_utf8_char()?),
            }
        }
        Err(VentoyConfigError::InvalidJson)
    }

    pub(super) fn consume_utf8_char(&mut self) -> Result<char, VentoyConfigError> {
        let start = self.pos.saturating_sub(1);
        let text = core::str::from_utf8(&self.input[start..])
            .map_err(|_| VentoyConfigError::InvalidJson)?;
        let ch = text.chars().next().ok_or(VentoyConfigError::InvalidJson)?;
        self.pos = start + ch.len_utf8();
        Ok(ch)
    }

    fn parse_unicode_escape(&mut self) -> Result<char, VentoyConfigError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let ch = self.next().ok_or(VentoyConfigError::InvalidJson)?;
            value = value
                .checked_mul(16)
                .and_then(|base| base.checked_add(hex_value(ch)?))
                .ok_or(VentoyConfigError::InvalidJson)?;
        }

        char::from_u32(value).ok_or(VentoyConfigError::InvalidJson)
    }

    pub(super) fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    pub(super) fn expect(&mut self, expected: u8) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(VentoyConfigError::InvalidJson)
        }
    }

    pub(super) fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }
}

fn hex_value(ch: u8) -> Option<u32> {
    match ch {
        b'0'..=b'9' => Some(u32::from(ch - b'0')),
        b'a'..=b'f' => Some(u32::from(ch - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(ch - b'A' + 10)),
        _ => None,
    }
}
