# ISO Compatibility Matrix

This matrix defines the UEFI-only ISO coverage that matters before NextBoot can
claim mainstream compatibility. Rows start as `planned`, move to `qemu-pass`
when an automated smoke exists, and move to `hardware-pass` only after a real
machine boots the image.

## Status Labels

| Status | Meaning |
| --- | --- |
| implemented | Boot path exists in code and is covered by synthetic or generated tests |
| qemu-pass | A representative image boots or reaches the expected smoke marker in QEMU |
| hardware-pass | A real machine booted the image successfully |
| partial | Some variants work, but a common path or host/device class still fails |
| planned | Required for mainstream compatibility but not yet proven |

## Mainstream UEFI Targets

| Family | Examples | Current status | Main path |
| --- | --- | --- | --- |
| Ubuntu/Casper | Ubuntu Desktop, Linux Mint, Pop!_OS | implemented | GRUB/EFI chain-load, casper kernel/initrd fallback |
| Debian Live/Installer | Debian Live, Debian netinst | implemented | GRUB/isolinux config parsing, `findiso` fallback |
| Fedora/RHEL style | Fedora Workstation, Rocky, Alma, CentOS Stream | implemented | BLS/GRUB config parsing, pxeboot kernel/initrd fallback |
| Arch style | Arch, Manjaro, BlackArch | implemented | GRUB config parsing, Arch kernel/initrd fallback |
| openSUSE style | openSUSE Leap/Tumbleweed | implemented | GRUB config parsing, loader kernel/initrd fallback |
| Generic Linux Live | SystemRescue, Alpine, Clonezilla, GParted, Parted Magic | partial | Generic EFI chain-load plus kernel/initrd candidates |
| Windows installer | Windows 10/11 ISO | implemented | Microsoft EFI chain-load plus WIMBOOT fallback |
| WinPE/Recovery | Hiren's, Sergei Strelec, Windows recovery media | partial | WIMBOOT fallback, helper assets, runtime injection |
| Hypervisor installers | Proxmox VE, ESXi, TrueNAS SCALE | planned | ISO EFI chain-load first, distro-specific fallback later |
| Router/appliance images | OpenWrt, pfSense, OPNsense, VyOS | planned | ISO EFI chain-load or raw IMG/VHD path |
| Raw disk images | Linux/utility `.img` | implemented | Virtual hard disk mapping |
| Virtual disks | VHD, VHDX, VDI | implemented | Virtual block device mapping, parent-chain diagnostics |

## Required Evidence Before Claiming "Mainstream ISO Compatible"

The first public compatibility target should contain at least:

- Ubuntu Desktop LTS.
- Debian Live.
- Fedora Workstation.
- Arch Linux.
- Linux Mint.
- openSUSE Tumbleweed or Leap.
- SystemRescue.
- Clonezilla.
- GParted Live.
- Windows 10 installer.
- Windows 11 installer.
- One common WinPE image.
- Proxmox VE installer.
- TrueNAS SCALE installer.
- OpenWrt x86 image or ISO-style installer where available.

Each row needs:

- Image name and version.
- SHA256 or upstream URL.
- Host flasher used.
- Media type and sector size where known.
- Firmware mode: UEFI, Secure Boot off unless the row says otherwise.
- Result: pass, partial, fail, blocked.
- Notes for required plugin/config/runtime assets.

## Compatibility Work Items

1. Add real ISO sample rows as images are tested.
2. Promote rows from `planned` to `qemu-pass` only with automated evidence.
3. Promote rows to `hardware-pass` only with a hardware report.
4. Add distro-specific fixes only after a failing real ISO is understood.
5. Keep Legacy BIOS out of this matrix; this is a UEFI-only product target.
