# Hardware Matrix Status

This file is generated from `docs/hardware/hardware-matrix.csv`.
Run `./scripts/hardware-matrix-report.py` after adding real hardware rows.

## Summary

| Field | Value |
| --- | --- |
| Source CSV | `docs/hardware/hardware-matrix.csv` |
| Data rows | 0 |
| Required coverage | 0/10 (0%) |
| Production hardware claim | blocked |
| Partial rows count as covered | no |

## Results

| Result | Rows |
| --- | ---: |
| pass | 0 |
| partial | 0 |
| fail | 0 |
| blocked | 0 |
| unknown | 0 |

## Required Coverage

| Requirement | Status | Matching evidence |
| --- | --- | --- |
| Internal SSD NVMe 512 exFAT ISO | missing | media=fixed, nvme; bus=nvme; sector=512; fs=exfat; image=iso |
| Internal SSD NVMe 4096 exFAT ISO | missing | media=fixed, nvme; bus=nvme; sector=4096; fs=exfat; image=iso |
| USB stick FAT32 ISO | missing | media=usb; bus=usb; sector=512; fs=fat32; image=iso |
| USB SSD enclosure NTFS Windows WIMBOOT | missing | media=usb, enclosure; bus=usb; sector=512; fs=ntfs; image=windows, wimboot |
| USB SSD enclosure 4096 exFAT VHDX | missing | media=usb, enclosure; bus=usb; sector=4096; fs=exfat; image=vhdx |
| SATA SSD NTFS ISO | missing | media=fixed, sata; bus=sata, ahci; sector=512; fs=ntfs; image=iso |
| SD reader FAT32 ISO | missing | media=sd; bus=sd; sector=512; fs=fat32; image=iso |
| Linux-prepared ext4 plugins | missing | media=fixed, nvme, usb, enclosure; bus=nvme, usb; sector=4096; fs=ext4; image=linux, plugins |
| Linux-prepared XFS VLNK ISO | missing | media=fixed, nvme, usb, enclosure; bus=nvme, usb; sector=4096; fs=xfs; image=vlnk, iso |
| UDF Windows ISO | missing | media=fixed, nvme, sata, usb, sd, enclosure, other; bus=nvme, sata, ahci, usb, sd, virtio, other; sector=512, 4096; fs=udf; image=windows, iso |

## Next Evidence To Collect

- Internal SSD NVMe 512 exFAT ISO
- Internal SSD NVMe 4096 exFAT ISO
- USB stick FAT32 ISO
- USB SSD enclosure NTFS Windows WIMBOOT
- USB SSD enclosure 4096 exFAT VHDX
- SATA SSD NTFS ISO
- SD reader FAT32 ISO
- Linux-prepared ext4 plugins
- Linux-prepared XFS VLNK ISO
- UDF Windows ISO
