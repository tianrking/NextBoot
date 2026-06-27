# Ventoy Gap Analysis

This document tracks what NextBoot has already borrowed from Ventoy-style
workflows and which gaps are still worth pursuing.

## Already Covered

- Standard GPT split layout: a FAT32 ESP plus a data partition, avoiding a
  USB-only assumption and matching fixed SSD/NVMe deployments.
- Data filesystems: FAT32, exFAT, and NTFS in both image generation and
  boot-time scanning paths.
- Storage buses in QEMU: virtio, NVMe, SATA, USB mass storage, and SDHCI SD.
  SD currently has disk generation and filesystem verification coverage; boot
  smoke is experimental because the tested OVMF firmware drops to the internal
  shell instead of booting from QEMU's SDHCI device.
- 4K Native fixed-disk coverage for buses that can expose logical block size
  overrides in QEMU. NVMe is the primary 4K fixed-disk smoke path; QEMU's AHCI
  `ide-hd` model is kept at 512B because it requires 512B discard granularity.
- Ventoy-compatible `.vlnk.iso` pointer files for images outside `/ISO`.
- Ventoy-style Linux plugin payloads for persistence, injection, DUD, and
  auto-install smoke coverage.
- Windows ISO chain loading plus WIMBOOT fallback asset integration.
- Auto memdisk and menu memdisk smoke paths.

## Useful Ventoy Ideas Still Open

- Secure Boot distribution: Ventoy provides a documented Secure Boot workflow;
  NextBoot still needs a signing/enrollment story before this is user-friendly.
- Broader image types: VHD/VHDX/IMG scanning exists in the product direction,
  but boot coverage is still much thinner than ISO/WIM flows.
- More filesystems: Ventoy covers a broader set of user storage formats. UDF is
  partially implemented here for ISO internals, but data partitions still focus
  on FAT32/exFAT/NTFS.
- Cross-architecture boot: current build and QEMU scripts target x86_64 UEFI.
  AArch64 UEFI support would need its own build artifact, smoke EFI, and QEMU
  firmware path.
- Compatibility database: Ventoy has years of device reports. NextBoot needs a
  structured hardware matrix for real SSD, NVMe enclosure, USB stick, SD reader,
  and motherboard firmware combinations.

## Current Priority

Keep the split GPT, filesystem, and bus matrix green. That is the foundation
for the user's main requirement: the boot flow should work from fixed SSD/NVMe,
USB mass storage, SD-style media, and 512B or 4K-sector devices instead of being
tied to old removable USB behavior.
