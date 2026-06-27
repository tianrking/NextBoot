# Hardware Compatibility Matrix

NextBoot's QEMU matrix proves repeatable boot behavior before hardware is
touched. This document defines the real-device evidence we still need to
collect so the project can grow a Ventoy-style compatibility database instead
of relying on a single USB-stick workflow.

## Report Flow

1. Build the UEFI binary with `./scripts/build.sh debug` or `release`.
2. Flash the target device with `./scripts/flash.sh`.
3. Boot the device on real firmware and record the result.
4. Run `./scripts/hardware-report.sh` from the same checkout to create a
   Markdown report and optionally append one CSV row.

Example:

```bash
./scripts/hardware-report.sh \
  --device /dev/disk4 \
  --media usb \
  --bus usb \
  --layout split \
  --data-fs exfat \
  --sector-size 512 \
  --image-type iso \
  --firmware "ThinkPad T14 Gen 3 UEFI 1.50" \
  --result pass \
  --append-csv docs/hardware/hardware-matrix.csv
```

## Matrix Fields

The CSV created by `scripts/hardware-report.sh --append-csv` uses these
columns:

| Field | Meaning |
| --- | --- |
| `timestamp` | UTC time of report generation |
| `commit` | NextBoot git commit tested |
| `branch` | Local branch name |
| `host_arch` | Host architecture that generated the report |
| `device` | Device path, model, or user-friendly label |
| `media` | `fixed`, `nvme`, `sata`, `usb`, `sd`, `enclosure`, or `other` |
| `bus` | Firmware-visible bus family |
| `layout` | `split` or `single` |
| `data_fs` | Data partition filesystem |
| `sector_size` | Device logical sector size |
| `image_type` | ISO/VLNK/IMG/VHD/VHDX/VDI/WIM coverage |
| `firmware` | Machine model and firmware version |
| `result` | `pass`, `fail`, `partial`, `blocked`, or `unknown` |
| `report` | Path to the generated Markdown report |
| `notes` | Short operator note |

## Required Hardware Coverage

Before claiming broad SSD/USB/SD readiness, collect at least these rows:

| Media | Bus | Sector | Data FS | Image Type |
| --- | --- | --- | --- | --- |
| Internal SSD | NVMe | 512 | exFAT | ISO |
| Internal SSD | NVMe | 4096 | exFAT | ISO |
| USB stick | USB mass storage | 512 | FAT32 | ISO |
| USB SSD enclosure | USB mass storage | 512 | NTFS | Windows ISO/WIMBOOT |
| USB SSD enclosure | USB mass storage | 4096 | exFAT | VHDX |
| SATA SSD | SATA/AHCI | 512 | NTFS | ISO |
| SD card | SD reader | 512 | FAT32 | ISO |
| Linux-prepared disk | NVMe or USB | 4096 | ext4 | Linux ISO plugins |
| Linux-prepared disk | NVMe or USB | 4096 | XFS | VLNK ISO |
| Optical-style image file | Any | 512 or 4096 | UDF | Windows ISO |

## Result Rules

- `pass`: firmware lists NextBoot, NextBoot scans the expected image, and the
  selected image reaches its smoke marker or real OS handoff.
- `partial`: one part worked but another required a workaround, such as manual
  firmware boot selection.
- `fail`: the device boots NextBoot but the intended workflow fails.
- `blocked`: host tools, firmware policy, or missing assets prevented a fair
  test.
- `unknown`: inventory report only, no boot attempt yet.

Keep raw serial logs, photos, or firmware screenshots next to the generated
Markdown report when possible. The CSV should stay small and reviewable; large
evidence belongs in per-run Markdown files under `target/hardware-reports/` or
an issue attachment.
