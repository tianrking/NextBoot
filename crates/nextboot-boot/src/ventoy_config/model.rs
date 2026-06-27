use super::md5::{md5_digest, parse_md5_hex};
use super::{
    find_matching_rule, normalize_config_path, path_parent_matches, path_pattern_eq,
    target_matches_image, JsonParser,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const VENTOY_JSON_MAX_SIZE: usize = 256 * 1024;
const VENTOY_MAX_CONF_REPLACE: usize = 2;

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
    pub(super) fn set_flag(&mut self, key: &str, enabled: bool) {
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
    pub(super) fn parse(value: &str) -> Option<Self> {
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
    pub auto_memdisk: bool,
}

impl VentoyImagePlugin {
    pub fn is_empty(&self) -> bool {
        self.auto_install.is_none()
            && self.persistence.is_none()
            && self.injection_archive.is_none()
            && self.dud.is_none()
            && self.conf_replace.is_empty()
            && !self.auto_memdisk
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
    pub windows_cd_prompt: bool,
    pub windows_uefi_resolution_lock: u8,
    pub windows11_bypass_check: bool,
    pub windows11_bypass_nro: bool,
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
    pub auto_memdisk: Vec<String>,
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
        plugin.auto_memdisk = self
            .auto_memdisk
            .iter()
            .any(|target| path_pattern_eq(target, path));

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
