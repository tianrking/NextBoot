# Release Media

NextBoot does not need an end-user flasher UI. The release artifact is a raw
media image plus a small burn tool.

## User Flow

1. Download `nextboot-all-uefi-universal-512b-exfat.img.xz` and
   `nextboot-tools.tar.gz`.
2. Extract the tools archive.
3. Run the burn tool:

```bash
./scripts/burn-release-media.sh \
  --image nextboot-all-uefi-universal-512b-exfat.img.xz \
  /dev/diskX
```

4. Open the new `NEXTDATA` partition.
5. Drag ISO, WIM, VHD, VHDX, IMG, or EFI files into `/ISO`.
6. Reboot and choose the device from the firmware UEFI boot menu.

The image already contains:

- A GPT partition table.
- A FAT32 ESP with `EFI/BOOT/BOOTX64.EFI`, `EFI/BOOT/BOOTIA32.EFI`, and
  `EFI/BOOT/BOOTAA64.EFI`.
- A growable exFAT Data partition labeled `NEXTDATA`.
- An empty `/ISO` directory for user boot images.
- `burn-release-media.sh`, which writes and expands release media in one step
  on macOS and Linux.

## Maintainer Build

```bash
./scripts/create-release-media.sh
```

The script builds the release UEFI binary, creates the raw media image, and
verifies that the ESP, Data partition, fallback loaders, and `/ISO` directory
are present. Public release builds use `--target all` so each image carries
x86_64, IA32, and AArch64 UEFI fallback loaders. Release media reserves exFAT
growth metadata. The burn tool writes the image and expands GPT plus
`NEXTDATA` immediately after writing, while the firmware first-boot grow path
remains a fallback for generic raw image writers. Optional `--image PATH`
arguments preseed boot images for QA builds; public release images should
normally ship empty so users can drag in their own files.
