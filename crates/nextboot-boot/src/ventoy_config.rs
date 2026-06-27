//! Minimal Ventoy `ventoy.json` plugin configuration support.
//!
//! Ventoy's plugin file is a broad JSON document. NextBoot consumes the parts
//! that directly shape image discovery and the first boot menu: global control
//! file filters, image white/black lists, menu aliases, and the boot-affecting
//! plugin metadata used later by Linux initrd and Windows runtime hooks.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VentoyPathTarget {
    Image(String),
    Parent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyPathRule<T> {
    pub target: VentoyPathTarget,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyAutoInstall {
    pub templates: Vec<String>,
    pub autosel: Option<usize>,
    pub timeout: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyPersistence {
    pub backends: Vec<String>,
    pub autosel: Option<usize>,
    pub timeout: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyDud {
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyConfReplace {
    pub img: Option<i32>,
    pub org: String,
    pub new_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VentoyImagePlugin {
    pub auto_install: Option<VentoyAutoInstall>,
    pub persistence: Option<VentoyPersistence>,
    pub injection_archive: Option<String>,
    pub dud: Option<VentoyDud>,
    pub conf_replace: Vec<VentoyConfReplace>,
}

impl VentoyImagePlugin {
    pub fn is_empty(&self) -> bool {
        self.auto_install.is_none()
            && self.persistence.is_none()
            && self.injection_archive.is_none()
            && self.dud.is_none()
            && self.conf_replace.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VentoyConfig {
    pub filters: VentoyFileFilters,
    pub default_search_root: Option<String>,
    pub image_list_mode: VentoyImageListMode,
    pub image_list: Vec<String>,
    pub menu_aliases: Vec<VentoyMenuAlias>,
    pub auto_install: Vec<VentoyPathRule<VentoyAutoInstall>>,
    pub persistence: Vec<VentoyPathRule<VentoyPersistence>>,
    pub injection: Vec<VentoyPathRule<String>>,
    pub dud: Vec<VentoyPathRule<VentoyDud>>,
    pub conf_replace: Vec<VentoyPathRule<VentoyConfReplace>>,
}

impl VentoyConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self, VentoyConfigError> {
        if bytes.len() > VENTOY_JSON_MAX_SIZE {
            return Err(VentoyConfigError::FileTooLarge);
        }

        let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &bytes[3..]
        } else if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
            return Err(VentoyConfigError::InvalidJson);
        } else {
            bytes
        };
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
                .any(|entry| path_pattern_eq(entry.as_str(), path)),
            VentoyImageListMode::Deny => !self
                .image_list
                .iter()
                .any(|entry| path_pattern_eq(entry.as_str(), path)),
        }
    }

    pub fn menu_alias_for(&self, path: &str) -> Option<&str> {
        self.menu_aliases
            .iter()
            .find(|entry| path_pattern_eq(entry.image.as_str(), path))
            .map(|entry| entry.alias.as_str())
    }

    pub fn image_plugin_for(&self, path: &str) -> Option<VentoyImagePlugin> {
        let mut plugin = VentoyImagePlugin::default();
        plugin.auto_install = find_matching_rule(&self.auto_install, path).cloned();
        plugin.persistence = find_matching_rule(&self.persistence, path).cloned();
        plugin.injection_archive = find_matching_rule(&self.injection, path).cloned();
        plugin.dud = find_matching_rule(&self.dud, path).cloned();

        for rule in &self.conf_replace {
            if target_matches_image(&rule.target, path) {
                plugin.conf_replace.try_reserve_exact(1).ok()?;
                plugin.conf_replace.push(rule.value.clone());
                if plugin.conf_replace.len() >= VENTOY_MAX_CONF_REPLACE {
                    break;
                }
            }
        }

        if plugin.is_empty() {
            None
        } else {
            Some(plugin)
        }
    }
}

const VENTOY_MAX_CONF_REPLACE: usize = 2;

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

fn find_matching_rule<'a, T>(rules: &'a [VentoyPathRule<T>], path: &str) -> Option<&'a T> {
    for rule in rules {
        if matches!(rule.target, VentoyPathTarget::Image(_))
            && target_matches_image(&rule.target, path)
        {
            return Some(&rule.value);
        }
    }

    for rule in rules {
        if matches!(rule.target, VentoyPathTarget::Parent(_))
            && target_matches_image(&rule.target, path)
        {
            return Some(&rule.value);
        }
    }

    None
}

fn target_matches_image(target: &VentoyPathTarget, path: &str) -> bool {
    match target {
        VentoyPathTarget::Image(pattern) => path_pattern_eq(pattern, path),
        VentoyPathTarget::Parent(parent) => path_parent_matches(parent, path),
    }
}

fn path_pattern_eq(pattern: &str, path: &str) -> bool {
    let pattern = normalize_config_path(pattern);
    let path = normalize_config_path(path);
    pattern_bytes_eq(pattern.as_bytes(), path.as_bytes())
}

fn path_parent_matches(parent: &str, path: &str) -> bool {
    let parent = normalize_config_path(parent);
    let path = normalize_config_path(path);
    let parent_bytes = parent.as_bytes();
    let path_bytes = path.as_bytes();

    if parent_bytes == b"/" {
        return path_bytes.starts_with(b"/") && !path_bytes[1..].contains(&b'/');
    }
    if parent_bytes.len() >= path_bytes.len() || path_bytes.get(parent_bytes.len()) != Some(&b'/') {
        return false;
    }
    if !pattern_bytes_eq(parent_bytes, &path_bytes[..parent_bytes.len()]) {
        return false;
    }

    !path_bytes[parent_bytes.len() + 1..].contains(&b'/')
}

fn pattern_bytes_eq(pattern: &[u8], path: &[u8]) -> bool {
    pattern.len() == path.len()
        && pattern
            .iter()
            .zip(path)
            .all(|(left, right)| *left == b'*' || ascii_byte_eq(*left, *right))
}

fn ascii_byte_eq(left: u8, right: u8) -> bool {
    if left.is_ascii() && right.is_ascii() {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn clean_plugin_paths(paths: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        if is_absolute_config_path(&path) && out.try_reserve_exact(1).is_ok() {
            out.push(normalize_config_path(&path));
        }
    }
    out
}

fn clean_plugin_path(path: String) -> Option<String> {
    is_absolute_config_path(&path).then(|| normalize_config_path(&path))
}

fn is_absolute_config_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\')
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
}

enum VentoyTargetKind {
    Image,
    Parent,
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
                "auto_install" => self.parse_auto_install(&mut config)?,
                "persistence" => self.parse_persistence(&mut config)?,
                "injection" => self.parse_injection(&mut config)?,
                "dud" => self.parse_dud(&mut config)?,
                "conf_replace" => self.parse_conf_replace(&mut config)?,
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

    fn parse_auto_install(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut templates = Vec::new();
            let mut autosel = None;
            let mut timeout = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "image" => {
                        if let Some(value) = parser.parse_target(VentoyTargetKind::Image)? {
                            target = Some(value);
                        }
                    }
                    "parent" => {
                        let value = parser.parse_target(VentoyTargetKind::Parent)?;
                        if target.is_none() {
                            target = value;
                        }
                    }
                    "template" => {
                        templates = parser.parse_path_list_value()?.unwrap_or_default();
                    }
                    "autosel" => autosel = parser.parse_optional_usize()?,
                    "timeout" => timeout = parser.parse_optional_u32()?,
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            let templates = clean_plugin_paths(templates);
            if let Some(target) = target {
                if !templates.is_empty() {
                    config
                        .auto_install
                        .try_reserve_exact(1)
                        .map_err(|_| VentoyConfigError::OutOfMemory)?;
                    config.auto_install.push(VentoyPathRule {
                        target,
                        value: VentoyAutoInstall {
                            templates,
                            autosel,
                            timeout,
                        },
                    });
                }
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

    fn parse_persistence(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut backends = Vec::new();
            let mut autosel = None;
            let mut timeout = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "image" => target = parser.parse_target(VentoyTargetKind::Image)?,
                    "backend" => {
                        backends = parser.parse_path_list_value()?.unwrap_or_default();
                    }
                    "autosel" => autosel = parser.parse_optional_usize()?,
                    "timeout" => timeout = parser.parse_optional_u32()?,
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            let backends = clean_plugin_paths(backends);
            if let Some(target) = target {
                if !backends.is_empty() {
                    config
                        .persistence
                        .try_reserve_exact(1)
                        .map_err(|_| VentoyConfigError::OutOfMemory)?;
                    config.persistence.push(VentoyPathRule {
                        target,
                        value: VentoyPersistence {
                            backends,
                            autosel,
                            timeout,
                        },
                    });
                }
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

    fn parse_injection(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut archive = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "image" => {
                        if let Some(value) = parser.parse_target(VentoyTargetKind::Image)? {
                            target = Some(value);
                        }
                    }
                    "parent" => {
                        let value = parser.parse_target(VentoyTargetKind::Parent)?;
                        if target.is_none() {
                            target = value;
                        }
                    }
                    "archive" => {
                        archive = parser.parse_optional_string()?.and_then(clean_plugin_path)
                    }
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            if let (Some(target), Some(archive)) = (target, archive) {
                config
                    .injection
                    .try_reserve_exact(1)
                    .map_err(|_| VentoyConfigError::OutOfMemory)?;
                config.injection.push(VentoyPathRule {
                    target,
                    value: archive,
                });
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

    fn parse_dud(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut files = Vec::new();
            self.parse_object_fields(|parser, key| {
                match key {
                    "image" => target = parser.parse_target(VentoyTargetKind::Image)?,
                    "dud" => files = parser.parse_path_list_value()?.unwrap_or_default(),
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            let files = clean_plugin_paths(files);
            if let Some(target) = target {
                if !files.is_empty() {
                    config
                        .dud
                        .try_reserve_exact(1)
                        .map_err(|_| VentoyConfigError::OutOfMemory)?;
                    config.dud.push(VentoyPathRule {
                        target,
                        value: VentoyDud { files },
                    });
                }
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

    fn parse_conf_replace(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut org = None;
            let mut new_path = None;
            let mut img = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "iso" => target = parser.parse_target(VentoyTargetKind::Image)?,
                    "org" => org = parser.parse_optional_string()?.and_then(clean_plugin_path),
                    "new" => new_path = parser.parse_optional_string()?.and_then(clean_plugin_path),
                    "img" => img = parser.parse_optional_i32()?,
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            if let (Some(target), Some(org), Some(new_path)) = (target, org, new_path) {
                config
                    .conf_replace
                    .try_reserve_exact(1)
                    .map_err(|_| VentoyConfigError::OutOfMemory)?;
                config.conf_replace.push(VentoyPathRule {
                    target,
                    value: VentoyConfReplace { img, org, new_path },
                });
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

    fn parse_object_fields<F>(&mut self, mut field: F) -> Result<(), VentoyConfigError>
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

    fn parse_target(
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

    fn parse_path_list_value(&mut self) -> Result<Option<Vec<String>>, VentoyConfigError> {
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

    fn parse_optional_usize(&mut self) -> Result<Option<usize>, VentoyConfigError> {
        match self.parse_optional_i32()? {
            Some(value) if value >= 0 => Ok(Some(value as usize)),
            _ => Ok(None),
        }
    }

    fn parse_optional_u32(&mut self) -> Result<Option<u32>, VentoyConfigError> {
        match self.parse_optional_i32()? {
            Some(value) if value >= 0 => Ok(Some(value as u32)),
            _ => Ok(None),
        }
    }

    fn parse_optional_i32(&mut self) -> Result<Option<i32>, VentoyConfigError> {
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

    #[test]
    fn accepts_utf8_bom_like_ventoy() {
        let json = b"\xEF\xBB\xBF{\"image_list\":[\"/ISO/a.iso\"]}";
        let config = VentoyConfig::parse(json).expect("config");

        assert!(config.allows_image_path("/ISO/a.iso"));
    }

    #[test]
    fn parses_boot_plugin_metadata_for_image() {
        let json = br#"{
            "auto_install": [
                {
                    "image": "/ISO/ubuntu.iso",
                    "template": ["/scripts/user-data", "/scripts/meta-data"],
                    "autosel": 1,
                    "timeout": 5
                }
            ],
            "persistence": [
                {
                    "image": "/ISO/ubuntu.iso",
                    "backend": "/persistence/ubuntu.dat"
                }
            ],
            "injection": [
                { "image": "/ISO/ubuntu.iso", "archive": "/inject/tools.tar.gz" }
            ],
            "dud": [
                { "image": "/ISO/rhel*.iso", "dud": ["/dud/dd.iso", "relative.img"] }
            ],
            "conf_replace": [
                { "iso": "/ISO/ubuntu.iso", "org": "/boot/grub/grub.cfg", "new": "/cfg/a.cfg", "img": 0 },
                { "iso": "/ISO/ubuntu.iso", "org": "/isolinux/txt.cfg", "new": "/cfg/b.cfg", "img": 1 },
                { "iso": "/ISO/ubuntu.iso", "org": "/extra.cfg", "new": "/cfg/c.cfg", "img": 2 }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");
        let plugin = config.image_plugin_for("/iso/UBUNTU.iso").expect("plugin");

        let auto = plugin.auto_install.expect("auto install");
        assert_eq!(auto.templates, ["/scripts/user-data", "/scripts/meta-data"]);
        assert_eq!(auto.autosel, Some(1));
        assert_eq!(auto.timeout, Some(5));
        assert_eq!(
            plugin.persistence.expect("persistence").backends,
            ["/persistence/ubuntu.dat"]
        );
        assert_eq!(
            plugin.injection_archive.as_deref(),
            Some("/inject/tools.tar.gz")
        );
        assert_eq!(plugin.conf_replace.len(), 2);

        let dud = config
            .image_plugin_for("/ISO/rhel8.iso")
            .expect("dud plugin");
        assert_eq!(dud.dud.expect("dud").files, ["/dud/dd.iso"]);
    }

    #[test]
    fn parent_plugins_match_only_direct_children() {
        let json = br#"{
            "injection": [
                { "parent": "/ISO", "archive": "/inject/all.tar" }
            ],
            "auto_install": [
                { "parent": "/", "template": "/autoinstall/root.ks" }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert_eq!(
            config
                .image_plugin_for("/ISO/linux.iso")
                .expect("direct child")
                .injection_archive
                .as_deref(),
            Some("/inject/all.tar")
        );
        assert!(config.image_plugin_for("/ISO/nested/linux.iso").is_none());
        assert!(config
            .image_plugin_for("/root.iso")
            .expect("root child")
            .auto_install
            .is_some());
    }

    #[test]
    fn image_target_wins_over_parent_field() {
        let json = br#"{
            "injection": [
                { "parent": "/ISO", "image": "/Other/tool.iso", "archive": "/inject/tool.tar" }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert!(config.image_plugin_for("/ISO/linux.iso").is_none());
        assert_eq!(
            config
                .image_plugin_for("/Other/tool.iso")
                .expect("image match")
                .injection_archive
                .as_deref(),
            Some("/inject/tool.tar")
        );
    }
}
