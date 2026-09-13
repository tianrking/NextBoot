# Release Media

This describes the current development builder. The inspected v0.0.3 public
image does not contain the runtime added below. See [release readiness](release-readiness.md).

The release artifact is a raw media image. On Windows, the matching
`nextboot-*-windows-writer.ps1` release asset is the preferred writer because it
writes the full image and compares full-range SHA-256 digests before reporting
success.

## User Flow

1. Download `nextboot-universal-uefi.img.xz`.
2. On Windows, download the matching `nextboot-*-windows-writer.ps1`, open an
   Administrator PowerShell, run `Unblock-File .\nextboot-*-windows-writer.ps1`,
   find the target number with `Get-Disk`, and run
   `./nextboot-*-windows-writer.ps1 -ImagePath .\nextboot-*.img -DiskNumber N`.
   The writer asks for the disk number again before erasing it, refuses system
   and boot disks, then reads the full written range and compares SHA-256
   digests.
3. On macOS or Linux, use balenaEtcher, Raspberry Pi Imager, Rufus, Win32 Disk
   Imager, GNOME Disks, or another raw-image writer on an 8GB-or-larger USB
   stick, USB SSD, SD card, or external SSD.
4. If the chosen flasher does not accept `.img.xz`, download
   `nextboot-universal-uefi.img.zip`, extract it, and select
   the extracted `.img`.
5. Open the new `NEXTDATA` partition. It must be exFAT and contain `ISO`. If
   Windows reports RAW, do not format it: re-write the image with the verified
   Windows writer.
6. Drag ISO, WIM, VHD, VHDX, IMG, or EFI files into `/ISO`.
7. Reboot and choose the device from the firmware UEFI boot menu.

Flashing must write the whole device. Copying the image file into an existing
USB volume will not work.

The image already contains:

- A GPT partition table.
- A 32MiB FAT ESP with `EFI/BOOT/BOOTX64.EFI`, `EFI/BOOT/BOOTIA32.EFI`, and
  `EFI/BOOT/BOOTAA64.EFI`.
- A growable exFAT Data partition labeled `NEXTDATA`.
- An empty `/ISO` directory for user boot images.
- Eight pinned compatibility runtime files under `/ventoy`, original licenses,
  patch attribution and resulting hashes. Preserve these files when adding ISOs.

Builds use Rust 1.98.1 and the tracked Cargo.lock. Runtime resources are checked
against `docs/runtime-assets.json`; cached corrupt files fail verification.
`--ventoy-assets DIR` imports matching upstream assets for offline builds.
`--without-runtime` is exclusively for developer fixtures and is not releasable.
`--target all` is required for the three fallback loaders listed above; a default
local invocation builds only x86_64.

Before a release, `scripts/package-release-sources.py --version VERSION` packages
the exact tracked NextBoot source and checksum-pinned upstream source archive.
Release automation includes both archives in the checksum manifest and assets.

## Maintainer Build

```bash
./scripts/create-release-media.sh
```

The script builds the release UEFI binary, creates the raw media image, and
verifies that the ESP, Data partition, fallback loaders, and `/ISO` directory
are present. Public release builds use `--target all` so each image carries
x86_64, IA32, and AArch64 UEFI fallback loaders. Release media reserves exFAT
growth metadata so firmware can expand GPT plus `NEXTDATA` on first boot after
the image is written to larger storage. Optional `--image PATH` arguments
preseed boot images for QA builds; public release images should normally ship
empty so users can drag in their own files.
