pub(super) const WINDOWS_BOOTMGFW_PATH: &str = "/efi/microsoft/boot/bootmgfw.efi";
pub(super) const WIMBOOT_MAX_CALLBACK_PATH: usize = 512;
pub(super) const WIMBOOT_BOOT_WIM_CALLBACK_PATH: &str = "nb-boot-wim";
pub(super) const WIMBOOT_BCD_CALLBACK_PATH: &str = "nb-bcd";
pub(super) const WIMBOOT_BOOT_SDI_CALLBACK_PATH: &str = "nb-boot-sdi";
pub(super) const WIMBOOT_BOOTMGFW_CALLBACK_PATH: &str = "nb-bootmgfw";
pub(super) const WIMBOOT_SELF_CALLBACK_PATH: &str = "nb-wimboot";
pub(super) const WIMBOOT_WINPESHL_CALLBACK_PATH: &str = "nb-winpeshl";
pub(super) const WIMBOOT_XZ_MAX_OUTPUT_SIZE: usize = 2 * 1024 * 1024;
pub(super) const VTOYJUMP_CANDIDATES: &[&str] = &["/ventoy/vtoyjump64.exe"];
pub(super) const VENTOY_COMMON_CPIO_CANDIDATES: &[&str] = &["/ventoy/ventoy.cpio"];
pub(super) const LINUX_CONFIG_MAX_SIZE: usize = 512 * 1024;
pub(super) const VENTOY_CONF_REPLACE_MAX_SIZE: usize = 1024 * 1024;
pub(super) const ISO9660_SECTOR_SIZE: u64 = 2048;
pub(super) const LINUX_GRUB_CONFIG_CANDIDATES: &[&str] = &[
    "/boot/grub/grub.cfg",
    "/boot/grub/loopback.cfg",
    "/grub/grub.cfg",
    "/EFI/BOOT/grub.cfg",
    "/efi/boot/grub.cfg",
    "/boot/grub/kernels.cfg",
];
pub(super) const LINUX_ISOLINUX_CONFIG_CANDIDATES: &[&str] = &[
    "/isolinux/isolinux.cfg",
    "/isolinux/syslinux.cfg",
    "/syslinux/syslinux.cfg",
    "/boot/isolinux/isolinux.cfg",
    "/boot/syslinux/syslinux.cfg",
    "/boot/isolinux/syslinux.cfg",
];
pub(super) const LINUX_LOADER_ENTRY_DIRS: &[&str] = &["/loader/entries", "/boot/loader/entries"];
pub(super) const LINUX_KERNEL_CANDIDATES: &[&str] = &[
    "/casper/vmlinuz",
    "/casper/vmlinuz.efi",
    "/casper/vmlinuz.efi.signed",
    "/vmlinuz",
    "/vmlinuz64",
    "/live/vmlinuz",
    "/live/vmlinuz1",
    "/live/vmlinuz2",
    "/boot/vmlinuz",
    "/boot/vmlinuz-x86_64",
    "/boot/vmlinuz-lts",
    "/boot/vmlinuz-virt",
    "/boot/linux",
    "/boot/linux26",
    "/boot/kernel",
    "/arch/boot/x86_64/vmlinuz-linux",
    "/blackarch/boot/x86_64/vmlinuz-linux",
    "/images/pxeboot/vmlinuz",
    "/images/pxeboot/vmlinuz64",
    "/boot/x86_64/loader/linux",
    "/isolinux/vmlinuz",
    "/boot/isolinux/vmlinuz",
    "/syslinux/vmlinuz",
    "/syslinux/linux",
    "/sysresccd/boot/x86_64/vmlinuz",
    "/sysresccd/boot/i686/vmlinuz",
    "/proxmox/boot/linux26",
    "/boot/grml/vmlinuz",
    "/grml64/full/vmlinuz",
    "/EFI/BOOT/vmlinuz",
];
pub(super) const LINUX_INITRD_CANDIDATES: &[&str] = &[
    "/boot/all.rdz",
    "/casper/initrd",
    "/casper/initrd.gz",
    "/casper/initrd.lz",
    "/casper/initrd.xz",
    "/casper/initrd-oem",
    "/boot/grub/initrd.xz",
    "/initrd.gz",
    "/initrd.xz",
    "/initrd.lz",
    "/slax/boot/initrfs.img",
    "/minios/boot/initrfs.img",
    "/pmagic/initrd.img",
    "/boot/initrd.xz",
    "/boot/initrd.gz",
    "/boot/initrd",
    "/boot/x86_64/loader/initrd",
    "/boot/initramfs-x86_64.img",
    "/boot/initramfs-lts",
    "/boot/initramfs-virt",
    "/boot/initrd26.img",
    "/boot/isolinux/initramfs_data64.cpio.gz",
    "/boot/initrd.img",
    "/isolinux/initrd.gz",
    "/images/pxeboot/initrd.img",
    "/images/pxeboot/initrd64.img",
    "/Setup/initrd.gz",
    "/isolinux/initramfs",
    "/boot/iniramfs.igz",
    "/initrd-x86_64",
    "/live/initrd.img",
    "/initrd.img",
    "/sysresccd/boot/x86_64/sysresccd.img",
    "/CDlinux/initrd",
    "/parabola/boot/x86_64/parabolaiso.img",
    "/parabola/boot/x86_64/initramfs-linux-libre.img",
    "/hyperbola/boot/x86_64/hyperiso.img",
    "/EFI/BOOT/initrd.img",
    "/initrd",
    "/live/initrd1",
    "/isolinux/initrd.img",
    "/syslinux/kernel/initramfs.gz",
    "/boot/rootfs.xz",
    "/arch/boot/x86_64/archiso.img",
    "/blackarch/boot/x86_64/archiso.img",
    "/blackarch/boot/x86_64/initramfs-linux.img",
    "/live/initrd2.img",
    "/live/initrd.xz",
    "/live/initrd.gz",
    "/live/initrd.lz",
    "/install.amd/initrd.gz",
    "/install.amd/gtk/initrd.gz",
    "/austrumi/initrd.gz",
    "/boot/initfs.x86_64-efi",
    "/boot/initfs.i386-pc",
    "/antiX/initrd.gz",
    "/360Disk/initrd.gz",
    "/porteus/initrd.xz",
    "/pyabr/boot/initrfs.img",
    "/initrd0.img",
    "/sysresccd/boot/i686/sysresccd.img",
    "/boot/full.cz",
    "/boot/grml/initrd.img",
    "/grml64/full/initrd.img",
    "/proxmox/boot/initrd.img",
    "/live/initrd",
    "/initramfs-linux.img",
    "/boot/isolinux/initrd.gz",
];
pub(super) const WIMBOOT_BCD_CANDIDATES: &[&str] = &[
    "/ventoy/common_bcd",
    "/ventoy/bcd",
    "/boot/bcd",
    "/efi/microsoft/boot/bcd",
];
pub(super) const WIMBOOT_COMPRESSED_BCD_CANDIDATES: &[&str] =
    &["/ventoy/common_bcd.xz", "/ventoy/bcd.xz"];
pub(super) const WIMBOOT_BOOT_SDI_CANDIDATES: &[&str] = &[
    "/boot/boot.sdi",
    "/2K10/FONTS/boot.sdi",
    "/SSTR/boot.sdi",
    "/ISPE/BOOT.SDI",
    "/boot/uqi.sdi",
    "/ISYL/boot.sdi",
    "/WEPE/WEPE.SDI",
];
pub(super) const WINDOWS_ISO_BOOT_WIM_CANDIDATES: &[&str] = &[
    "/sources/boot.wim",
    "/boot/boot.wim",
    "/x64/sources/boot.wim",
    "/x86/sources/boot.wim",
];
pub(super) const WINDOWS_ISO_BCD_CANDIDATES: &[&str] = &[
    "/boot/bcd",
    "/efi/microsoft/boot/bcd",
    "/x64/boot/bcd",
    "/x86/boot/bcd",
];
pub(super) const WINDOWS_ISO_BOOT_SDI_CANDIDATES: &[&str] = &[
    "/boot/boot.sdi",
    "/x64/boot/boot.sdi",
    "/x86/boot/boot.sdi",
    "/2K10/FONTS/boot.sdi",
    "/SSTR/boot.sdi",
    "/ISPE/BOOT.SDI",
    "/boot/uqi.sdi",
    "/ISYL/boot.sdi",
    "/WEPE/WEPE.SDI",
];
pub(super) const WIMBOOT_BOOTMGFW_VIRTUAL_NAME: &str = "bootmgfw.efi";
pub(super) const WIMBOOT_WINPESHL_VIRTUAL_NAME: &str = "winpeshl.exe";
pub(super) const WIMBOOT_WIM_BOOTMGFW_CANDIDATES: &[&str] = &["\\Windows\\Boot\\EFI\\bootmgfw.efi"];
pub(super) const WIMBOOT_WIM_BCD_CANDIDATES: &[&str] = &["\\Windows\\Boot\\DVD\\EFI\\BCD"];
pub(super) const WIMBOOT_WIM_BOOT_SDI_CANDIDATES: &[&str] = &[
    "\\Windows\\Boot\\DVD\\EFI\\boot.sdi",
    "\\sms\\boot\\boot.sdi",
];
pub(super) const WIMBOOT_WIM_WINPESHL_CANDIDATES: &[&str] = &["\\Windows\\System32\\winpeshl.exe"];

const EFI_BOOT_X64: &str = "\\EFI\\BOOT\\BOOTX64.EFI";
const EFI_BOOT_AA64: &str = "\\EFI\\BOOT\\BOOTAA64.EFI";
const EFI_BOOT_IA32: &str = "\\EFI\\BOOT\\BOOTIA32.EFI";
const EFI_BOOT_ARM: &str = "\\EFI\\BOOT\\BOOTARM.EFI";

pub(super) fn default_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }

    #[cfg(target_arch = "arm")]
    {
        &[EFI_BOOT_ARM, EFI_BOOT_AA64, EFI_BOOT_X64, EFI_BOOT_IA32]
    }

    #[cfg(target_arch = "x86")]
    {
        &[EFI_BOOT_IA32, EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_ARM]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[EFI_BOOT_X64, EFI_BOOT_AA64, EFI_BOOT_IA32, EFI_BOOT_ARM]
    }
}

pub(super) fn generic_efi_boot_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &[
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(target_arch = "arm")]
    {
        &[
            "/efi/boot/bootarm.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootia32.efi",
        ]
    }

    #[cfg(target_arch = "x86")]
    {
        &[
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootarm.efi",
        ]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
    {
        &[
            "/efi/boot/bootx64.efi",
            "/efi/boot/bootaa64.efi",
            "/efi/boot/bootia32.efi",
            "/efi/boot/bootarm.efi",
        ]
    }
}

pub(super) fn wimboot_helper_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "x86_64")]
    {
        &[
            "/ventoy/wimboot.x86_64",
            "/ventoy/wimboot.x86_64.efi",
            "/ventoy/wimboot_x64.efi",
            "/ventoy/wimboot.efi",
        ]
    }

    #[cfg(target_arch = "x86")]
    {
        &[
            "/ventoy/wimboot.i386.efi",
            "/ventoy/wimboot.i386",
            "/ventoy/wimboot_ia32.efi",
            "/ventoy/wimboot.efi",
        ]
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        &[]
    }
}

pub(super) fn compressed_wimboot_helper_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "x86_64")]
    {
        &["/ventoy/wimboot.x86_64.xz"]
    }

    #[cfg(target_arch = "x86")]
    {
        &["/ventoy/wimboot.i386.efi.xz"]
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        &[]
    }
}

pub(super) fn ventoy_arch_cpio_candidates() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    {
        &["/ventoy/ventoy_arm64.cpio"]
    }

    #[cfg(target_arch = "x86")]
    {
        &["/ventoy/ventoy_x86.cpio"]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86")))]
    {
        &["/ventoy/ventoy_x86.cpio"]
    }
}
