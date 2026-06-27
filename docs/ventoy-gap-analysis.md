# Ventoy Gap Analysis

This document tracks what NextBoot has already borrowed from Ventoy-style
workflows and which gaps are still worth pursuing.

## Already Covered

- Standard GPT split layout: a FAT32 ESP plus a data partition, avoiding a
  USB-only assumption and matching fixed SSD/NVMe deployments.
- Data filesystems: FAT32, exFAT, ext2, ext3, ext4, NTFS, UDF, and a limited
  XFS QEMU subset in image generation, boot-time scanning paths, and the real
  flash workflow where host tooling can safely format and populate the
  partition.
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
- Windows ISO chain loading plus WIMBOOT fallback asset integration.
- Auto memdisk and menu memdisk smoke paths.
- Raw `.img`, fixed VHD, dynamic VHD, VHDX, and dynamic VDI virtual hard-disk
  boot smoke coverage with an inner GPT/FAT32 ESP.

## Useful Ventoy Ideas Still Open

- Secure Boot distribution: Ventoy provides a documented Secure Boot workflow;
  NextBoot still needs a signing/enrollment story before this is user-friendly.
- Broader image types: raw IMG, fixed/dynamic VHD, VHDX, and dynamic VDI have
  virtual hard-disk boot smoke coverage now; differencing VHDX/VDI parent
  chains and partially present sparse states still need a compatibility story.
- More filesystems: Ventoy covers a broader set of user storage formats. UDF,
  ext2/3/4, and a first XFS extent-reading framework are now covered in the
  QEMU data partition and flash paths; real `mkfs.xfs` directory formats still
  need broader compatibility work.
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
