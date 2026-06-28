# Release Media

NextBoot does not need an end-user flasher UI. The release artifact is a raw
media image that users can burn with their usual imaging tool.

## User Flow

1. Download `nextboot-x86_64-unknown-uefi-512b-exfat.img`.
2. Burn the `.img` to a USB stick, USB SSD, SD card, or external SSD.
3. Open the new `NEXTDATA` partition.
4. Drag ISO, WIM, VHD, VHDX, IMG, or EFI files into `/ISO`.
5. Reboot and choose the device from the firmware UEFI boot menu.

The image already contains:

- A GPT partition table.
- A FAT32 ESP with `EFI/BOOT/BOOTX64.EFI`.
- An exFAT Data partition labeled `NEXTDATA`.
- An empty `/ISO` directory for user boot images.

## Maintainer Build

```bash
./scripts/create-release-media.sh
```

The script builds the release UEFI binary, creates the raw media image, and
verifies that the ESP, Data partition, fallback loader, and `/ISO` directory are
present. Optional `--image PATH` arguments preseed boot images for QA builds;
public release images should normally ship empty so users can drag in their own
files.
