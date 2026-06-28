use super::{
    clean_default_image_path, is_current_platform_plugin_key, parse_u32_text,
    parse_windows_uefi_resolution_lock, JsonParser, VentoyConfig, VentoyConfigError,
    VentoyImageListMode, VentoyPassword, VentoyPasswordConfig, VentoyPathRule, VentoyTargetKind,
};
use alloc::vec::Vec;

impl<'a> JsonParser<'a> {
    pub(super) fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }
    pub(super) fn parse_config(&mut self) -> Result<VentoyConfig, VentoyConfigError> {
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
}
