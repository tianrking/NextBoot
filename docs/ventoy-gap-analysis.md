# Ventoy Gap Analysis

This document tracks what NextBoot has already borrowed from Ventoy-style
workflows and which gaps are still worth pursuing.

## Already Covered

- Standard GPT split layout: a FAT32 ESP plus a data partition, avoiding a
  USB-only assumption and matching fixed SSD/NVMe deployments.
- Data filesystems: FAT32, exFAT, ext2, ext3, ext4, NTFS, UDF, a limited XFS
  QEMU subset, and a limited Btrfs QEMU subset in image generation and
  boot-time scanning paths. Real flash workflows cover formats where host
  tooling can safely format and populate the partition.
- Storage buses in QEMU: virtio, NVMe, SATA, USB mass storage, and SDHCI SD.
  SD currently has disk generation and filesystem verification coverage; boot
  smoke is experimental because the tested OVMF firmware drops to the internal
  shell instead of booting from QEMU's SDHCI device.
- 4K Native fixed-disk coverage for buses that can expose logical block size
  overrides in QEMU. NVMe is the primary 4K fixed-disk smoke path; QEMU's AHCI
  `ide-hd` model is kept at 512B because it requires 512B discard granularity.
- Ventoy-compatible `.vlnk.iso` pointer files for images outside `/ISO`,
  including XFS QEMU data partition smoke coverage.
- Ventoy-style Linux plugin payloads for persistence, injection, DUD, and
  auto-install smoke coverage.
- Ventoy-style `conf_replace` ISO file replacement rules now keep practical
  batches of up to 32 matching replacements instead of truncating after two.
  A QEMU NVMe 4K exFAT smoke case verifies three replacement files are parsed
  from `ventoy.json`, loaded from the data partition, and prepared as virtual
  ISO overlays before the El Torito EFI payload starts.
- Windows ISO chain loading plus WIMBOOT fallback asset integration.
- Auto memdisk and menu memdisk smoke paths.
- Raw `.img`, fixed VHD, dynamic VHD, VHDX, and dynamic/static VDI virtual
  hard-disk boot smoke coverage with an inner GPT/FAT32 ESP.
- Sparse VHDX `ZERO` BAT entries, self-contained VHDX `PARTIALLY_PRESENT`
  entries with full sector bitmaps, and sparse VDI unallocated or discarded
  block-map entries in virtual hard-disk boot smoke coverage.
- VHDX and VDI virtual disk metadata parsing is isolated in the host-testable
  `nextboot-image` crate. VHDX Parent Locator key/value metadata and VDI
  differencing UUID linkage fields are parsed and covered by unit tests, giving
  parent-chain readers a verified way to identify parent candidates instead of
  treating all differencing images as opaque failures.
- The same host-tested image crate now plans virtual disk spans for VHDX and
  VDI child images, classifying every virtual range as child-image data,
  parent-backed data, or zero fill. The boot layer can now resolve same-volume
  VHDX Parent Locator paths and use one parent VHDX for fully parent-backed
  missing blocks and mixed `PARTIALLY_PRESENT` child/parent sectors. The QEMU
  matrix now boots parent-backed VHDX children with same-volume parents present.
  VDI differencing images can now resolve a same-directory parent by matching
  linkage UUIDs and boot through that parent fallback in QEMU. Multi-level
  VHDX/VDI parent chains are bounded to eight parents in firmware and covered
  by four-parent QEMU smoke cases. Missing-parent VHDX/VDI cases are also
  smoke-tested so users get the missing candidate path or same-directory UUID
  recovery hint before the boot fails with `UNSUPPORTED`.
- Local Secure Boot signing workflow: `scripts/secure-boot.sh` can generate a
  local test certificate, sign the UEFI binary with sbsigntools, verify the
  signed binary where `sbverify` is available, and document firmware db or shim
  MOK enrollment via `docs/secure-boot.md`. The checked-in
  `docs/secure-boot-release-policy.json` is also validated by CI, so public
  Secure Boot readiness cannot be claimed until shim or Microsoft UEFI CA
  status, SBAT/revocation policy, release-key custody, and authenticated update
  generation are filled in.
- IA32 and AArch64 UEFI test paths: `TARGET=i686-unknown-uefi` and
  `TARGET=aarch64-unknown-uefi` now build the bootloader and smoke EFI, write
  `EFI/BOOT/BOOTIA32.EFI` or `EFI/BOOT/BOOTAA64.EFI` into generated QEMU disks
  and flash media, verify those fallback paths, and run under `qemu-system-i386`
  with edk2-i386 firmware or `qemu-system-aarch64` with AAVMF/edk2-aarch64
  firmware.
- Cross-architecture release media: `TARGET=all` builds x86_64, IA32, and
  AArch64 UEFI artifacts, and both `flash.sh --target all` and the GitHub
  release image install `EFI/BOOT/BOOTX64.EFI`, `EFI/BOOT/BOOTIA32.EFI`, and
  `EFI/BOOT/BOOTAA64.EFI` on one ESP for portable SSD, USB, or SD media.
- Customer-burnable release media: `scripts/create-release-media.sh` creates a
  raw GPT image with a FAT32 ESP, multi-architecture UEFI fallback loaders, an
  exFAT `NEXTDATA` partition, and an empty `/ISO` directory. GitHub releases
  publish one universal image instead of 8GB/32GB capacity tiers. The release
  image reserves growable exFAT metadata, and NextBoot can expand GPT plus
  `NEXTDATA` after the image is written to larger USB, SSD, or SD media.
  Users flash the image with normal raw-image writers such as balenaEtcher,
  Raspberry Pi Imager, Rufus, Win32 Disk Imager, or GNOME Disks, then drag boot
  images into `/ISO`. Firmware first-boot growth handles larger target media
  because generic flashers do not rewrite GPT/exFAT for the destination size.

## Useful Ventoy Ideas Still Open

- Legacy BIOS boot: Ventoy supports BIOS and UEFI. NextBoot is intentionally
  UEFI-first today, so BIOS-only machines still need either a compatibility
  loader story or a clear out-of-scope decision before full Ventoy parity can
  be claimed.
- Secure Boot distribution: local owner-key signing is documented now, but
  NextBoot still needs a production-grade shim or Microsoft UEFI CA story,
  SBAT/revocation policy, release key management, and authenticated variable
  update generation before public Secure Boot distribution is user-friendly.
  Those blockers are now represented in a CI-validated release policy instead
  of only prose.
- Broader image types: raw IMG, fixed/dynamic VHD, VHDX, sparse VHDX,
  self-contained partially-present VHDX, dynamic/static VDI, and sparse VDI
  unallocated/discarded blocks have virtual hard-disk boot smoke coverage now;
  differencing VHDX/VDI metadata and parent locator/linkage fields are parsed
  in host tests, and child/parent/zero spans are planned in reusable code. VHDX
  can now open and boot from a same-volume one-level parent for both missing
  blocks and mixed partial-bitmaps, and VDI can boot through a same-directory
  one-level parent selected by UUID linkage. Both formats now resolve tested
  multi-level parent chains up to a bounded depth, and missing-parent
  diagnostics are covered in QEMU. More automated recovery or locator rewrite
  tooling can still improve the user experience.
- More filesystems: Ventoy covers a broader set of user storage formats. UDF,
  ext2/3/4, XFS extent/local-directory/dir2-directory reads, and a Btrfs
  superblock plus NextBoot extent/directory smoke map are now covered in QEMU
  data partition paths. Real `mkfs.xfs` btree-scale directories and real
  `mkfs.btrfs` checksum/B-tree metadata still need broader compatibility work.
- Compatibility database: Ventoy has years of device reports. NextBoot now has
  `scripts/hardware-report.sh` and `docs/hardware-compatibility-matrix.md` to
  collect structured rows for real SSD, NVMe enclosure, USB stick, SD reader,
  and motherboard firmware combinations, but the matrix still needs real-world
  entries before it can be considered mature.
- Advanced menu/plugin polish: menu aliases, tips, classes, passwords, and the
  boot-affecting plugin data are parsed or partly enforced, but full Ventoy
  theme parity, richer menu UX, and more plugin choice flows are still lower
  priority than boot correctness.

## Current Priority

Keep the split GPT, filesystem, and bus matrix green. That is the foundation
for the user's main requirement: the boot flow should work from fixed SSD/NVMe,
USB mass storage, SD-style media, and 512B or 4K-sector devices instead of being
tied to old removable USB behavior.
