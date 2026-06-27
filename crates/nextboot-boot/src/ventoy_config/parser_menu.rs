use super::{
    JsonParser, VentoyConfig, VentoyConfigError, VentoyMenuAlias, VentoyMenuClassRule,
    VentoyMenuClassTarget, VentoyMenuTip, VentoyMenuTipRule, VentoyMenuTipTarget,
};
use alloc::string::String;
use alloc::vec::Vec;

impl<'a> JsonParser<'a> {
    pub(super) fn parse_menu_tip(
        &mut self,
        config: &mut VentoyConfig,
    ) -> Result<(), VentoyConfigError> {
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
    pub(super) fn parse_menu_class(
        &mut self,
        config: &mut VentoyConfig,
    ) -> Result<(), VentoyConfigError> {
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
    pub(super) fn parse_menu_alias(
        &mut self,
        config: &mut VentoyConfig,
    ) -> Result<(), VentoyConfigError> {
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
}
