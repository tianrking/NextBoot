//! Minimal Ventoy `ventoy.json` plugin configuration support.
//!
//! Ventoy's plugin file is a broad JSON document. NextBoot consumes the parts
//! that directly shape image discovery and the first boot menu: global control
//! file filters, image white/black lists, menu aliases, and the boot-affecting
//! plugin metadata used later by Linux initrd and Windows runtime hooks.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

mod md5;
mod model;

pub use model::*;

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

fn clean_default_image_path(path: &str) -> Option<String> {
    let path = strip_default_image_hotkey(path).trim();
    clean_plugin_path(path.to_string())
}

fn strip_default_image_hotkey(path: &str) -> &str {
    let trimmed = path.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() > 3
        && matches!(bytes[0], b'F' | b'f')
        && matches!(bytes[1], b'2'..=b'9')
        && bytes[2] == b'>'
    {
        &trimmed[3..]
    } else {
        trimmed
    }
}

fn is_absolute_config_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\')
}

fn parse_u32_text(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut value = 0u32;
    for byte in trimmed.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    Some(value)
}

fn parse_windows_uefi_resolution_lock(text: &str) -> u8 {
    match text.trim() {
        "1" => 1,
        "2" => 2,
        _ => 0,
    }
}

#[cfg(target_arch = "aarch64")]
const VENTOY_PLATFORM_SUFFIX: &str = "aa64";
#[cfg(target_arch = "x86_64")]
const VENTOY_PLATFORM_SUFFIX: &str = "uefi";
#[cfg(target_arch = "x86")]
const VENTOY_PLATFORM_SUFFIX: &str = "ia32";
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "x86")))]
const VENTOY_PLATFORM_SUFFIX: &str = "uefi";

fn is_current_platform_plugin_key(key: &str, base: &str) -> bool {
    key.strip_prefix(base)
        .and_then(|suffix| suffix.strip_prefix('_'))
        == Some(VENTOY_PLATFORM_SUFFIX)
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
        let mut parsed_platform_password = false;
        let mut parsed_platform_menu_tip = false;
        let mut parsed_platform_menu_class = false;
        let mut parsed_platform_auto_memdisk = false;
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
                key if is_current_platform_plugin_key(key, "menu_tip") => {
                    self.parse_menu_tip(&mut config)?;
                    parsed_platform_menu_tip = true;
                }
                "menu_tip" if !parsed_platform_menu_tip => self.parse_menu_tip(&mut config)?,
                key if is_current_platform_plugin_key(key, "menu_class") => {
                    self.parse_menu_class(&mut config)?;
                    parsed_platform_menu_class = true;
                }
                "menu_class" if !parsed_platform_menu_class => {
                    self.parse_menu_class(&mut config)?
                }
                "password_uefi" | "password_aa64" => {
                    self.parse_password(&mut config)?;
                    parsed_platform_password = true;
                }
                "password" if !parsed_platform_password => self.parse_password(&mut config)?,
                "auto_install" => self.parse_auto_install(&mut config)?,
                "persistence" => self.parse_persistence(&mut config)?,
                "injection" => self.parse_injection(&mut config)?,
                "dud" => self.parse_dud(&mut config)?,
                "conf_replace" => self.parse_conf_replace(&mut config)?,
                key if is_current_platform_plugin_key(key, "auto_memdisk") => {
                    self.parse_auto_memdisk(&mut config)?;
                    parsed_platform_auto_memdisk = true;
                }
                "auto_memdisk" if !parsed_platform_auto_memdisk => {
                    self.parse_auto_memdisk(&mut config)?
                }
                "image_list" => {
                    config.image_list_mode = VentoyImageListMode::Allow;
                    config.image_list = self.parse_string_array()?;
                }
                "image_blacklist" => {
                    config.image_list_mode = VentoyImageListMode::Deny;
                    config.image_list = self.parse_string_array()?;
                }
                "menu_tip" | "menu_class" | "auto_memdisk" => self.skip_value()?,
                "password" => self.skip_value()?,
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
                    match key.as_str() {
                        "VTOY_DEFAULT_SEARCH_ROOT" if !value.is_empty() => {
                            config.default_search_root = Some(value);
                        }
                        "VTOY_MAX_SEARCH_LEVEL" => {
                            config.max_search_level = parse_u32_text(&value)
                                .and_then(|level| usize::try_from(level).ok());
                        }
                        "VTOY_MENU_TIMEOUT" => {
                            config.menu_timeout = parse_u32_text(&value);
                        }
                        "VTOY_DEFAULT_IMAGE" => {
                            config.default_image = clean_default_image_path(&value);
                        }
                        "VTOY_DEFAULT_MENU_MODE" => {
                            config.default_menu_mode = parse_u32_text(&value);
                        }
                        "VTOY_LINUX_REMOUNT" => {
                            config.linux_remount = value == "1";
                        }
                        "VTOY_WINDOWS_CD_PROMPT" => {
                            config.windows_cd_prompt = value == "1";
                        }
                        "VTOY_WIN_UEFI_RES_LOCK" => {
                            config.windows_uefi_resolution_lock =
                                parse_windows_uefi_resolution_lock(&value);
                        }
                        "VTOY_WIN11_BYPASS_CHECK" => {
                            config.windows11_bypass_check = value == "1";
                        }
                        "VTOY_WIN11_BYPASS_NRO" => {
                            config.windows11_bypass_nro = value == "1";
                        }
                        "VTOY_FILT_DOT_UNDERSCORE_FILE" => {
                            config.filter_dot_underscore = value == "1";
                        }
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

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        Ok(())
    }

    fn parse_menu_tip(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        let mut tips = Vec::new();

        self.parse_object_fields(|parser, key| {
            match key {
                "tips" => tips = parser.parse_menu_tip_rules()?,
                _ => parser.skip_value()?,
            }
            Ok(())
        })?;

        config.menu_tips = tips;
        Ok(())
    }

    fn parse_menu_tip_rules(&mut self) -> Result<Vec<VentoyMenuTipRule>, VentoyConfigError> {
        let mut rules = Vec::new();
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut tip1 = String::new();
            let mut tip2 = String::new();
            self.parse_object_fields(|parser, key| {
                match key {
                    "image" => {
                        if let Some(path) = parser.parse_optional_config_path()? {
                            target = Some(VentoyMenuTipTarget::Image(path));
                        }
                    }
                    "dir" => {
                        if target.is_none() {
                            if let Some(path) = parser.parse_optional_config_path()? {
                                target = Some(VentoyMenuTipTarget::Dir(path));
                            }
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    "tip" | "tip1" => {
                        if let Some(value) = parser.parse_optional_string()? {
                            tip1 = value;
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    "tip2" => {
                        if let Some(value) = parser.parse_optional_string()? {
                            tip2 = value;
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            let tip = VentoyMenuTip { tip1, tip2 };
            if let Some(target) = target {
                if !tip.is_empty() {
                    rules
                        .try_reserve_exact(1)
                        .map_err(|_| VentoyConfigError::OutOfMemory)?;
                    rules.push(VentoyMenuTipRule { target, tip });
                }
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        Ok(rules)
    }

    fn parse_menu_class(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        let mut rules = Vec::new();
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut class = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "key" => {
                        if let Some(value) = parser.parse_optional_string()? {
                            target = Some(VentoyMenuClassTarget::Key(value));
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    "parent" => {
                        if target.is_none() {
                            if let Some(value) = parser.parse_optional_config_path()? {
                                target = Some(VentoyMenuClassTarget::Parent(value));
                            }
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    "dir" => {
                        if target.is_none() {
                            if let Some(value) = parser.parse_optional_string()? {
                                target = Some(VentoyMenuClassTarget::Dir(value));
                            } else {
                                parser.skip_value()?;
                            }
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    "class" => {
                        if let Some(value) = parser.parse_optional_string()? {
                            class = Some(value);
                        } else {
                            parser.skip_value()?;
                        }
                    }
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            if let (Some(target), Some(class)) = (target, class) {
                if !class.is_empty() {
                    rules
                        .try_reserve_exact(1)
                        .map_err(|_| VentoyConfigError::OutOfMemory)?;
                    rules.push(VentoyMenuClassRule { target, class });
                }
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        config.menu_classes = rules;
        Ok(())
    }

    fn parse_password(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        let mut password = VentoyPasswordConfig::default();

        self.parse_object_fields(|parser, key| {
            match key {
                "bootpwd" => {
                    password.boot = parser.parse_optional_password()?;
                }
                "isopwd" => {
                    password.iso = parser.parse_optional_password()?;
                }
                "wimpwd" => {
                    password.wim = parser.parse_optional_password()?;
                }
                "efipwd" => {
                    password.efi = parser.parse_optional_password()?;
                }
                "imgpwd" => {
                    password.img = parser.parse_optional_password()?;
                }
                "vhdpwd" => {
                    password.vhd = parser.parse_optional_password()?;
                }
                "vtoypwd" => {
                    password.vtoy = parser.parse_optional_password()?;
                }
                "menupwd" => {
                    password.menu = parser.parse_password_rules()?;
                }
                _ => parser.skip_value()?,
            }
            Ok(())
        })?;

        config.password = password;
        Ok(())
    }

    fn parse_password_rules(
        &mut self,
    ) -> Result<Vec<VentoyPathRule<VentoyPassword>>, VentoyConfigError> {
        let mut rules = Vec::new();
        self.skip_ws();
        self.expect(b'[')?;
        loop {
            self.skip_ws();
            if self.consume(b']') {
                break;
            }

            let mut target = None;
            let mut password = None;
            self.parse_object_fields(|parser, key| {
                match key {
                    "file" => {
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
                    "pwd" => {
                        password = parser.parse_optional_password()?;
                    }
                    _ => parser.skip_value()?,
                }
                Ok(())
            })?;

            if let (Some(target), Some(password)) = (target, password) {
                rules
                    .try_reserve_exact(1)
                    .map_err(|_| VentoyConfigError::OutOfMemory)?;
                rules.push(VentoyPathRule {
                    target,
                    value: password,
                });
            }

            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }

        Ok(rules)
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

    fn parse_auto_memdisk(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
        let paths = clean_plugin_paths(self.parse_string_array()?);
        config
            .auto_memdisk
            .try_reserve_exact(paths.len())
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        config.auto_memdisk.extend(paths);
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

    fn parse_optional_password(&mut self) -> Result<Option<VentoyPassword>, VentoyConfigError> {
        match self.parse_optional_string()? {
            Some(value) => Ok(VentoyPassword::parse(value.as_str())),
            None => {
                self.skip_value()?;
                Ok(None)
            }
        }
    }

    fn parse_optional_config_path(&mut self) -> Result<Option<String>, VentoyConfigError> {
        match self.parse_optional_string()? {
            Some(value) => Ok(clean_plugin_path(value)),
            None => {
                self.skip_value()?;
                Ok(None)
            }
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
mod tests;
