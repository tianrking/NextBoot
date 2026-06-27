use super::*;

#[test]
fn parses_core_ventoy_plugins() {
    let json = br#"{
            "control": [
                { "VTOY_FILE_FLT_WIM": "1" },
                { "VTOY_DEFAULT_SEARCH_ROOT": "/ISO" },
                { "VTOY_MAX_SEARCH_LEVEL": "2" },
                { "VTOY_LINUX_REMOUNT": "1" },
                { "VTOY_WINDOWS_CD_PROMPT": "1" },
                { "VTOY_WIN_UEFI_RES_LOCK": "2" },
                { "VTOY_WIN11_BYPASS_CHECK": "1" },
                { "VTOY_WIN11_BYPASS_NRO": "1" },
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
    assert!(config.windows_cd_prompt);
    assert_eq!(config.windows_uefi_resolution_lock, 2);
    assert!(config.windows11_bypass_check);
    assert!(config.windows11_bypass_nro);
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
fn maps_windows_uefi_resolution_lock_like_ventoy_reserved_flag() {
    let lock_one =
        VentoyConfig::parse(br#"{"control":[{"VTOY_WIN_UEFI_RES_LOCK":"1"}]}"#).expect("lock one");
    let lock_two =
        VentoyConfig::parse(br#"{"control":[{"VTOY_WIN_UEFI_RES_LOCK":"2"}]}"#).expect("lock two");
    let default_mode = VentoyConfig::parse(br#"{"control":[{"VTOY_WIN_UEFI_RES_LOCK":"3"}]}"#)
        .expect("default mode");
    let invalid = VentoyConfig::parse(br#"{"control":[{"VTOY_WIN_UEFI_RES_LOCK":"bad"}]}"#)
        .expect("invalid mode");

    assert_eq!(lock_one.windows_uefi_resolution_lock, 1);
    assert_eq!(lock_two.windows_uefi_resolution_lock, 2);
    assert_eq!(default_mode.windows_uefi_resolution_lock, 0);
    assert_eq!(invalid.windows_uefi_resolution_lock, 0);
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
            "auto_memdisk": [
                "/ISO/ubuntu.iso"
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
    assert!(plugin.auto_memdisk);

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
