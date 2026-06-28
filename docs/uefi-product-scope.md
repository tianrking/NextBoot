# UEFI Product Scope

NextBoot is intentionally UEFI-first. Legacy BIOS support is out of scope for
the current product line, and production Secure Boot distribution is deferred
until the UEFI boot path and ISO compatibility matrix are proven on real
hardware.

## Product Target

The release should feel like this to a user:

1. Download a NextBoot release image.
2. Flash it with a normal raw-image writer.
3. Open `NEXTDATA`.
4. Drag ISO/WIM/VHD/VHDX/IMG/EFI files into `/ISO`.
5. Reboot through the firmware UEFI boot menu.
6. Pick the desired image in NextBoot.

No project-specific command line should be required for the first install.

## Priority Order

| Priority | Area | Requirement |
| --- | --- | --- |
| P0 | UEFI release media | Standard raw image, multi-arch fallback loaders, visible `NEXTDATA` |
| P0 | Mainstream ISO compatibility | Ubuntu, Debian, Fedora, Arch, Windows, WinPE, rescue, hypervisor, and appliance images |
| P0 | Non-destructive update | Upgrade NextBoot loaders without deleting existing `/ISO` content |
| P1 | Real hardware evidence | USB stick, USB SSD, SD reader, SATA SSD, NVMe enclosure, 512B and 4K paths |
| P1 | Failure diagnostics | Clear unsupported-image and missing-resource messages |
| P2 | Secure Boot distribution | Public shim or Microsoft UEFI CA path, SBAT, revocation policy |
| Out | Legacy BIOS | Not planned for the current UEFI-only product line |

## Plugins

In Ventoy terminology, a plugin is not a browser-style extension. It is a
configuration entry, usually in `ventoy/ventoy.json`, that changes how a boot
image is presented or booted.

Useful plugin categories for NextBoot:

- Menu metadata: aliases, classes, tips, default image, timeout, and passwords.
- Linux boot behavior: persistence, injection archives, driver update disks,
  and auto-install answers.
- Windows boot behavior: unattended files, driver/runtime injection, Windows
  11 check bypass controls, and WIMBOOT helpers.
- ISO overlays: `conf_replace` rules that virtually replace files inside an
  ISO before boot.

Users do not need to understand these for the default drag-and-boot flow. The
bootloader still needs the underlying support because many real-world ISO
workflows depend on persistence, automated installation, or injected drivers.

## Non-Destructive Update

Install and update are different operations:

- Install writes a whole raw image to a device and erases that device.
- Update replaces only the NextBoot UEFI loader files in the ESP.

The update operation must preserve:

- The `NEXTDATA` partition.
- Existing `/ISO` files.
- User plugin/config files.
- User-created folders on the data partition.

The developer-facing implementation is `scripts/update-media.sh`. It mounts
only the ESP and updates `EFI/BOOT/BOOTX64.EFI`, `EFI/BOOT/BOOTIA32.EFI`, and
`EFI/BOOT/BOOTAA64.EFI` as requested. A future GUI updater can call the same
operation, but the important product guarantee is that update never partitions,
formats, or rewrites `NEXTDATA`.

## Definition Of Done For UEFI-Only Maturity

NextBoot can be called mature for the current scope when all of these are true:

- The release image boots on the required real hardware matrix.
- The mainstream ISO matrix has pass or documented-partial rows.
- Non-destructive update is validated on macOS, Windows, and Linux host flows.
- Failures produce actionable messages instead of silent menu exits.
- Secure Boot is either explicitly unsupported in release notes or implemented
  through a production-grade signing path.
