//! Minimal Ventoy `ventoy.json` plugin configuration support.
//!
//! Ventoy's plugin file is a broad JSON document. NextBoot consumes the parts
//! that directly shape image discovery and the first boot menu: global control
//! file filters, image white/black lists, and menu aliases.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

const VENTOY_JSON_MAX_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyConfigError {
    NotFound,
    FileTooLarge,
    InvalidJson,
    OutOfMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VentoyFileFilters {
    pub iso: bool,
    pub wim: bool,
    pub efi: bool,
    pub img: bool,
    pub vhd: bool,
    pub vtoy: bool,
}

impl VentoyFileFilters {
    fn set_flag(&mut self, key: &str, enabled: bool) {
        match key {
            "VTOY_FILE_FLT_ISO" => self.iso = enabled,
            "VTOY_FILE_FLT_WIM" => self.wim = enabled,
            "VTOY_FILE_FLT_EFI" => self.efi = enabled,
            "VTOY_FILE_FLT_IMG" => self.img = enabled,
            "VTOY_FILE_FLT_VHD" => self.vhd = enabled,
            "VTOY_FILE_FLT_VTOY" => self.vtoy = enabled,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyImageListMode {
    None,
    Allow,
    Deny,
}

impl Default for VentoyImageListMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyMenuAlias {
    pub image: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VentoyConfig {
    pub filters: VentoyFileFilters,
    pub default_search_root: Option<String>,
    pub image_list_mode: VentoyImageListMode,
    pub image_list: Vec<String>,
    pub menu_aliases: Vec<VentoyMenuAlias>,
}

impl VentoyConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self, VentoyConfigError> {
        if bytes.len() > VENTOY_JSON_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let text = core::str::from_utf8(bytes).map_err(|_| VentoyConfigError::InvalidJson)?;
        let mut parser = JsonParser::new(text);
        parser.parse_config()
    }

    pub fn search_roots<'a>(&'a self, fallback: &'a [&'a str]) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(root) = self
            .default_search_root
            .as_deref()
            .map(normalize_config_path)
            .filter(|path| !path.is_empty())
        {
            if out.try_reserve_exact(1).is_ok() {
                out.push(root);
            }
            return out;
        }

        if out.try_reserve_exact(fallback.len()).is_ok() {
            for path in fallback {
                out.push((*path).to_string());
            }
        }
        out
    }

    pub fn supports_image_name(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".iso") {
            return !self.filters.iso;
        }
        if lower.ends_with(".wim") || lower.ends_with(".esd") {
            return !self.filters.wim;
        }
        if lower.ends_with(".efi") {
            return !self.filters.efi;
        }
        if lower.ends_with(".img") {
            return !self.filters.img;
        }
        if lower.ends_with(".vhd") || lower.ends_with(".vhdx") || lower.ends_with(".vdi") {
            return !self.filters.vhd;
        }
        if lower.ends_with(".vtoy") {
            return !self.filters.vtoy;
        }

        false
    }

    pub fn allows_image_path(&self, path: &str) -> bool {
        match self.image_list_mode {
            VentoyImageListMode::None => true,
            VentoyImageListMode::Allow => self
                .image_list
                .iter()
                .any(|entry| path_eq(entry.as_str(), path)),
            VentoyImageListMode::Deny => !self
                .image_list
                .iter()
                .any(|entry| path_eq(entry.as_str(), path)),
        }
    }

    pub fn menu_alias_for(&self, path: &str) -> Option<&str> {
        self.menu_aliases
            .iter()
            .find(|entry| path_eq(entry.image.as_str(), path))
            .map(|entry| entry.alias.as_str())
    }
}

fn normalize_config_path(path: &str) -> String {
    let mut out = String::new();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return out;
    }
    if !trimmed.starts_with('/') && !trimmed.starts_with('\\') {
        out.push('/');
    }
    for ch in trimmed.chars() {
        out.push(if ch == '\\' { '/' } else { ch });
    }
    out
}

fn path_eq(left: &str, right: &str) -> bool {
    normalize_config_path(left).eq_ignore_ascii_case(&normalize_config_path(right))
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn parse_config(&mut self) -> Result<VentoyConfig, VentoyConfigError> {
        let mut config = VentoyConfig::default();
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
            match key.as_str() {
                "control" => self.parse_control(&mut config)?,
                "menu_alias" => self.parse_menu_alias(&mut config)?,
                "image_list" => {
                    config.image_list_mode = VentoyImageListMode::Allow;
                    config.image_list = self.parse_string_array()?;
                }
                "image_blacklist" => {
                    config.image_list_mode = VentoyImageListMode::Deny;
                    config.image_list = self.parse_string_array()?;
                }
                _ => self.skip_value()?,
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b'}')?;
            break;
        }

        self.skip_ws();
        if self.pos == self.input.len() {
            Ok(config)
        } else {
            Err(VentoyConfigError::InvalidJson)
        }
    }

    fn parse_control(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            self.expect(b'{')?;
            loop {
                self.skip_ws();
                if self.consume(b'}') {
                    break;
                }

                let key = self.parse_string()?;
                self.skip_ws();
                self.expect(b':')?;
                if let Some(value) = self.parse_optional_string()? {
                    config.filters.set_flag(key.as_str(), value == "1");
                    if key == "VTOY_DEFAULT_SEARCH_ROOT" && !value.is_empty() {
                        config.default_search_root = Some(value);
                    }
                } else {
                    self.skip_value()?;
                }

                self.skip_ws();
                if self.consume(b',') {
                    continue;
                }
                self.expect(b'}')?;
                break;
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        Ok(())
    }

    fn parse_menu_alias(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut image = None;
            let mut alias = None;
            self.expect(b'{')?;
            loop {
                self.skip_ws();
                if self.consume(b'}') {
                    break;
                }

                let key = self.parse_string()?;
                self.skip_ws();
                self.expect(b':')?;
                if let Some(value) = self.parse_optional_string()? {
                    match key.as_str() {
                        "image" => image = Some(value),
                        "alias" => alias = Some(value),
                        _ => {}
                    }
                } else {
                    self.skip_value()?;
                }

                self.skip_ws();
                if self.consume(b',') {
                    continue;
                }
                self.expect(b'}')?;
                break;
            }

            if let (Some(image), Some(alias)) = (image, alias) {
                config
                    .menu_aliases
                    .try_reserve_exact(1)
                    .map_err(|_| VentoyConfigError::OutOfMemory)?;
                config.menu_aliases.push(VentoyMenuAlias { image, alias });
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        Ok(())
    }

    fn parse_string_array(&mut self) -> Result<Vec<String>, VentoyConfigError> {
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

    fn parse_optional_string(&mut self) -> Result<Option<String>, VentoyConfigError> {
        self.skip_ws();
        if self.peek() == Some(b'"') {
            return self.parse_string().map(Some);
        }
        Ok(None)
    }

    fn skip_value(&mut self) -> Result<(), VentoyConfigError> {
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

    fn parse_string(&mut self) -> Result<String, VentoyConfigError> {
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

    fn consume_utf8_char(&mut self) -> Result<char, VentoyConfigError> {
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

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(VentoyConfigError::InvalidJson)
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_ventoy_plugins() {
        let json = br#"{
            "control": [
                { "VTOY_FILE_FLT_WIM": "1" },
                { "VTOY_DEFAULT_SEARCH_ROOT": "/ISO" }
            ],
            "menu_alias": [
                { "image": "/ISO/win11.iso", "alias": "Windows 11" }
            ],
            "image_blacklist": ["/ISO/old.iso"]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert!(config.filters.wim);
        assert_eq!(config.default_search_root.as_deref(), Some("/ISO"));
        assert_eq!(config.image_list_mode, VentoyImageListMode::Deny);
        assert!(!config.allows_image_path("/iso/old.iso"));
        assert!(config.allows_image_path("/iso/win11.iso"));
        assert_eq!(config.menu_alias_for("/iso/WIN11.ISO"), Some("Windows 11"));
        assert!(!config.supports_image_name("boot.wim"));
    }

    #[test]
    fn parses_image_whitelist_and_escaped_alias() {
        let json = br#"{
            "menu_alias": [
                { "image": "\\ISO\\linux.iso", "alias": "Linux \u0031" }
            ],
            "image_list": ["/ISO/linux.iso"]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert_eq!(config.image_list_mode, VentoyImageListMode::Allow);
        assert!(config.allows_image_path("/iso/linux.iso"));
        assert!(!config.allows_image_path("/iso/other.iso"));
        assert_eq!(config.menu_alias_for("/ISO/linux.iso"), Some("Linux 1"));
    }

    #[test]
    fn preserves_utf8_menu_alias() {
        let json =
            "{\"menu_alias\":[{\"image\":\"/ISO/tools.iso\",\"alias\":\"\u{5de5}\u{5177}\u{7bb1}\"}]}";

        let config = VentoyConfig::parse(json.as_bytes()).expect("config");

        assert_eq!(
            config.menu_alias_for("/iso/tools.iso"),
            Some("\u{5de5}\u{5177}\u{7bb1}")
        );
    }

    #[test]
    fn rejects_trailing_json_garbage() {
        let err = VentoyConfig::parse(br#"{} trailing"#).expect_err("invalid json");

        assert_eq!(err, VentoyConfigError::InvalidJson);
    }
}
