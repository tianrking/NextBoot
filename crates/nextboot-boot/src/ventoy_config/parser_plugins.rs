use super::{
    clean_plugin_path, clean_plugin_paths, JsonParser, VentoyAutoInstall, VentoyConfReplace,
    VentoyConfig, VentoyConfigError, VentoyDud, VentoyPathRule, VentoyPersistence,
    VentoyTargetKind,
};
use alloc::vec::Vec;

impl<'a> JsonParser<'a> {
    pub(super) fn parse_auto_install(
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
    pub(super) fn parse_auto_memdisk(
        &mut self,
        config: &mut VentoyConfig,
    ) -> Result<(), VentoyConfigError> {
        let paths = clean_plugin_paths(self.parse_string_array()?);
        config
            .auto_memdisk
            .try_reserve_exact(paths.len())
            .map_err(|_| VentoyConfigError::OutOfMemory)?;
        config.auto_memdisk.extend(paths);
        Ok(())
    }
    pub(super) fn parse_persistence(
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
    pub(super) fn parse_injection(
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
    pub(super) fn parse_dud(&mut self, config: &mut VentoyConfig) -> Result<(), VentoyConfigError> {
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
    pub(super) fn parse_conf_replace(
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
}
