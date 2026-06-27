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
pub struct VentoyMenuTip {
    pub tip1: String,
    pub tip2: String,
}

impl VentoyMenuTip {
    pub fn is_empty(&self) -> bool {
        self.tip1.is_empty() && self.tip2.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VentoyMenuTipTarget {
    Image(String),
    Dir(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyMenuTipRule {
    pub target: VentoyMenuTipTarget,
    pub tip: VentoyMenuTip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VentoyMenuClassTarget {
    Key(String),
    Parent(String),
    Dir(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VentoyMenuClassRule {
    pub target: VentoyMenuClassTarget,
    pub class: String,
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
pub enum VentoyPassword {
    Text(String),
    Md5([u8; 16]),
    SaltedMd5 { salt: String, digest: [u8; 16] },
}

impl VentoyPassword {
    fn parse(value: &str) -> Option<Self> {
        if value.len() > 64 {
            return None;
        }

        if let Some(text) = value.strip_prefix("txt#") {
            return Some(Self::Text(text.to_string()));
        }

        let md5 = value.strip_prefix("md5#")?;
        if md5.len() == 32 {
            return parse_md5_hex(md5).map(Self::Md5);
        }

        let (salt, digest) = md5.split_once('#')?;
        if digest.len() != 32 {
            return None;
        }

        Some(Self::SaltedMd5 {
            salt: salt.to_string(),
            digest: parse_md5_hex(digest)?,
        })
    }

    pub fn verify(&self, input: &str) -> bool {
        match self {
            Self::Text(expected) => expected == input,
            Self::Md5(expected) => md5_digest(input.as_bytes()) == *expected,
            Self::SaltedMd5 { salt, digest } => {
                let mut data = Vec::new();
                if data
                    .try_reserve_exact(salt.len().saturating_add(input.len()))
                    .is_err()
                {
                    return false;
                }
                data.extend_from_slice(salt.as_bytes());
                data.extend_from_slice(input.as_bytes());
                md5_digest(&data) == *digest
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VentoyPasswordConfig {
    pub boot: Option<VentoyPassword>,
    pub iso: Option<VentoyPassword>,
    pub wim: Option<VentoyPassword>,
    pub efi: Option<VentoyPassword>,
    pub img: Option<VentoyPassword>,
    pub vhd: Option<VentoyPassword>,
    pub vtoy: Option<VentoyPassword>,
    pub menu: Vec<VentoyPathRule<VentoyPassword>>,
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
    pub filter_dot_underscore: bool,
    pub default_search_root: Option<String>,
    pub max_search_level: Option<usize>,
    pub menu_timeout: Option<u32>,
    pub default_image: Option<String>,
    pub default_menu_mode: Option<u32>,
    pub linux_remount: bool,
    pub image_list_mode: VentoyImageListMode,
    pub image_list: Vec<String>,
    pub menu_aliases: Vec<VentoyMenuAlias>,
    pub menu_tips: Vec<VentoyMenuTipRule>,
    pub menu_classes: Vec<VentoyMenuClassRule>,
    pub password: VentoyPasswordConfig,
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

    pub fn menu_tip_for_image(&self, path: &str) -> Option<&VentoyMenuTip> {
        self.menu_tips
            .iter()
            .find(|entry| match &entry.target {
                VentoyMenuTipTarget::Image(pattern) => path_pattern_eq(pattern.as_str(), path),
                VentoyMenuTipTarget::Dir(_) => false,
            })
            .map(|entry| &entry.tip)
            .filter(|tip| !tip.is_empty())
    }

    pub fn menu_class_for_image(&self, path: &str) -> Option<&str> {
        let filename = path.rsplit('/').next().unwrap_or(path);

        for rule in &self.menu_classes {
            if let VentoyMenuClassTarget::Key(pattern) = &rule.target {
                if pattern.len() < filename.len() && filename.contains(pattern.as_str()) {
                    return Some(rule.class.as_str());
                }
            }
        }

        for rule in &self.menu_classes {
            if let VentoyMenuClassTarget::Parent(parent) = &rule.target {
                if path_parent_matches(parent.as_str(), path) {
                    return Some(rule.class.as_str());
                }
            }
        }

        None
    }

    pub fn default_image_matches(&self, path: &str) -> bool {
        self.default_image
            .as_ref()
            .is_some_and(|default| path_pattern_eq(default.as_str(), path))
    }

    pub fn image_password_for(&self, path: &str) -> Option<&VentoyPassword> {
        find_matching_rule(&self.password.menu, path).or_else(|| self.password_for_image_type(path))
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

    fn password_for_image_type(&self, path: &str) -> Option<&VentoyPassword> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".iso") {
            self.password.iso.as_ref()
        } else if lower.ends_with(".wim") || lower.ends_with(".esd") {
            self.password.wim.as_ref()
        } else if lower.ends_with(".efi") {
            self.password.efi.as_ref()
        } else if lower.ends_with(".img") {
            self.password.img.as_ref()
        } else if lower.ends_with(".vhd") || lower.ends_with(".vhdx") || lower.ends_with(".vdi") {
            self.password.vhd.as_ref()
        } else if lower.ends_with(".vtoy") {
            self.password.vtoy.as_ref()
        } else {
            None
        }
    }
}

const VENTOY_MAX_CONF_REPLACE: usize = 2;

fn parse_md5_hex(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }

    let mut out = [0u8; 16];
    for (index, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
        out[index] = hex_byte(chunk[0])?
            .checked_mul(16)?
            .checked_add(hex_byte(chunk[1])?)?;
    }
    Some(out)
}

fn hex_byte(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut message = Vec::new();
    let padding_len = if input.len() % 64 < 56 {
        56 - input.len() % 64
    } else {
        120 - input.len() % 64
    };
    if message
        .try_reserve_exact(input.len().saturating_add(padding_len).saturating_add(8))
        .is_err()
    {
        return [0; 16];
    }
    message.extend_from_slice(input);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0 = 0x6745_2301u32;
    let mut b0 = 0xefcd_ab89u32;
    let mut c0 = 0x98ba_dcfeu32;
    let mut d0 = 0x1032_5476u32;

    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };

            let next = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(words[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(next.rotate_left(S[i]));
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());
    digest
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
                "image_list" => {
                    config.image_list_mode = VentoyImageListMode::Allow;
                    config.image_list = self.parse_string_array()?;
                }
                "image_blacklist" => {
                    config.image_list_mode = VentoyImageListMode::Deny;
                    config.image_list = self.parse_string_array()?;
                }
                "menu_tip" | "menu_class" => self.skip_value()?,
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
mod tests {
    use super::*;

    #[test]
    fn parses_core_ventoy_plugins() {
        let json = br#"{
            "control": [
                { "VTOY_FILE_FLT_WIM": "1" },
                { "VTOY_DEFAULT_SEARCH_ROOT": "/ISO" },
                { "VTOY_MAX_SEARCH_LEVEL": "2" },
                { "VTOY_LINUX_REMOUNT": "1" },
                { "VTOY_FILT_DOT_UNDERSCORE_FILE": "1" }
            ],
            "menu_alias": [
                { "image": "/ISO/win11.iso", "alias": "Windows 11" }
            ],
            "image_blacklist": ["/ISO/old.iso"]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert!(config.filters.wim);
        assert!(config.filter_dot_underscore);
        assert!(config.linux_remount);
        assert_eq!(config.default_search_root.as_deref(), Some("/ISO"));
        assert_eq!(config.max_search_level, Some(2));
        assert_eq!(config.image_list_mode, VentoyImageListMode::Deny);
        assert!(!config.allows_image_path("/iso/old.iso"));
        assert!(config.allows_image_path("/iso/win11.iso"));
        assert_eq!(config.menu_alias_for("/iso/WIN11.ISO"), Some("Windows 11"));
        assert!(!config.supports_image_name("boot.wim"));
    }

    #[test]
    fn treats_max_search_level_max_as_unlimited() {
        let json = br#"{
            "control": [
                { "VTOY_MAX_SEARCH_LEVEL": "max" }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert_eq!(config.max_search_level, None);
    }

    #[test]
    fn parses_menu_default_controls() {
        let json = br#"{
            "control": [
                { "VTOY_MENU_TIMEOUT": "8" },
                { "VTOY_DEFAULT_IMAGE": "F4>\\ISO\\Win11.iso" },
                { "VTOY_DEFAULT_MENU_MODE": "1" }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert_eq!(config.menu_timeout, Some(8));
        assert_eq!(config.default_image.as_deref(), Some("/ISO/Win11.iso"));
        assert_eq!(config.default_menu_mode, Some(1));
        assert!(config.default_image_matches("/iso/win11.iso"));
        assert!(!config.default_image_matches("/iso/ubuntu.iso"));
    }

    #[test]
    fn parses_menu_tip_and_class_plugins() {
        let json = br#"{
            "menu_tip": {
                "left": "5%",
                "tips": [
                    { "image": "/ISO/ubuntu.iso", "tip": "Daily installer" },
                    { "dir": "/ISO/tools", "tip1": "Tools", "tip2": "Diagnostics" }
                ]
            },
            "menu_class": [
                { "key": "ubuntu", "class": "ubuntu" },
                { "parent": "/ISO", "class": "iso-root" },
                { "dir": "tools", "class": "folder-tools" }
            ]
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert_eq!(
            config.menu_tip_for_image("/iso/UBUNTU.iso"),
            Some(&VentoyMenuTip {
                tip1: "Daily installer".to_string(),
                tip2: String::new(),
            })
        );
        assert!(config.menu_tip_for_image("/ISO/tools/rescue.iso").is_none());
        assert_eq!(
            config.menu_class_for_image("/ISO/ubuntu.iso"),
            Some("ubuntu")
        );
        assert_eq!(
            config.menu_class_for_image("/ISO/rescue.iso"),
            Some("iso-root")
        );
        assert!(config.menu_class_for_image("/Other/rescue.iso").is_none());
    }

    #[test]
    fn parses_and_matches_password_plugin() {
        let json = br#"{
            "password": {
                "isopwd": "txt#fallback",
                "wimpwd": "md5#5ebe2294ecd0e0f08eab7690d2a6ee69",
                "menupwd": [
                    { "parent": "/ISO", "pwd": "txt#parent" },
                    { "file": "/ISO/special.iso", "pwd": "txt#special" }
                ]
            }
        }"#;

        let config = VentoyConfig::parse(json).expect("config");

        assert!(config
            .image_password_for("/iso/special.iso")
            .expect("file password")
            .verify("special"));
        assert!(config
            .image_password_for("/iso/ubuntu.iso")
            .expect("parent password")
            .verify("parent"));
        assert!(config
            .image_password_for("/tools/other.iso")
            .expect("type password")
            .verify("fallback"));
        assert!(config
            .image_password_for("/boot/install.wim")
            .expect("md5 password")
            .verify("secret"));
    }

    #[test]
    fn verifies_salted_md5_password() {
        let password =
            VentoyPassword::parse("md5#pepper#afcd70a1438b9b8ce9be72e89ca602a8").expect("password");

        assert!(password.verify("secret"));
        assert!(!password.verify("other"));
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
