use super::{parse_grub_cfg, parse_isolinux_cfg};
use alloc::string::String;

#[test]
fn grub_parser_extracts_first_menuentry_paths() {
    let cfg = r#"
        menuentry 'Try Ubuntu' {
            linuxefi (loop)/casper/vmlinuz boot=casper quiet splash ---
            initrdefi (loop)/casper/initrd
        }
    "#;

    assert_eq!(
        parse_grub_cfg(cfg),
        Some((
            String::from("/casper/vmlinuz"),
            String::from("/casper/initrd"),
            String::from("boot=casper quiet splash ---")
        ))
    );
}

#[test]
fn grub_parser_uses_last_initrd_component() {
    let cfg = r#"
        linux /arch/boot/x86_64/vmlinuz-linux archisobasedir=arch
        initrd /intel-ucode.img /arch/boot/x86_64/initramfs-linux.img
    "#;

    assert_eq!(
        parse_grub_cfg(cfg),
        Some((
            String::from("/arch/boot/x86_64/vmlinuz-linux"),
            String::from("/arch/boot/x86_64/initramfs-linux.img"),
            String::from("archisobasedir=arch")
        ))
    );
}

#[test]
fn grub_parser_uses_bls_options_line() {
    let cfg = r#"
        title Fedora Live
        options root=live:CDLABEL=Fedora quiet rhgb
        linux /images/pxeboot/vmlinuz
        initrd /images/pxeboot/initrd.img
    "#;

    assert_eq!(
        parse_grub_cfg(cfg),
        Some((
            String::from("/images/pxeboot/vmlinuz"),
            String::from("/images/pxeboot/initrd.img"),
            String::from("root=live:CDLABEL=Fedora quiet rhgb")
        ))
    );
}

#[test]
fn isolinux_parser_extracts_append_initrd_and_removes_duplicate_arg() {
    let cfg = r#"
        label live
          kernel vmlinuz
          append initrd=initrd.img boot=live quiet
    "#;

    assert_eq!(
        parse_isolinux_cfg(cfg),
        Some((
            String::from("vmlinuz"),
            String::from("initrd.img"),
            String::from("boot=live quiet")
        ))
    );
}

#[test]
fn grub_parser_expands_set_variables_and_device_prefixes() {
    let cfg = r#"
        set root='(cd0)'
        set kernel_path="/casper/vmlinuz"
        set initrd_path="/casper/initrd"
        menuentry "Try real-world Linux" {
            linuxefi ($root)$kernel_path boot=casper quiet splash ---
            initrdefi ($root)$initrd_path
        }
    "#;

    assert_eq!(
        parse_grub_cfg(cfg),
        Some((
            String::from("/casper/vmlinuz"),
            String::from("/casper/initrd"),
            String::from("boot=casper quiet splash ---")
        ))
    );
}

#[test]
fn grub_parser_ignores_inline_comments_and_keeps_quoted_cmdline_tokens() {
    let cfg = r#"
        menuentry 'SystemRescue' {
            linux /sysresccd/boot/x86_64/vmlinuz archisobasedir=sysresccd "cow_label=NEXTDATA" # comment
            initrd /sysresccd/boot/x86_64/sysresccd.img # trailing comment
        }
    "#;

    assert_eq!(
        parse_grub_cfg(cfg),
        Some((
            String::from("/sysresccd/boot/x86_64/vmlinuz"),
            String::from("/sysresccd/boot/x86_64/sysresccd.img"),
            String::from("archisobasedir=sysresccd cow_label=NEXTDATA")
        ))
    );
}

#[test]
fn isolinux_parser_handles_quoted_append_and_inline_comments() {
    let cfg = r#"
        label clonezilla
          kernel "/live/vmlinuz"
          append initrd="/live/initrd.img" boot=live "union=overlay" # ignored
    "#;

    assert_eq!(
        parse_isolinux_cfg(cfg),
        Some((
            String::from("/live/vmlinuz"),
            String::from("/live/initrd.img"),
            String::from("boot=live union=overlay")
        ))
    );
}
