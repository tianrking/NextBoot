#!/usr/bin/env bash
# Run a practical QEMU smoke matrix across fixed, removable, and SD-style media.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODE="${NEXTBOOT_QEMU_MODE:-debug}"
TIMEOUT="${NEXTBOOT_QEMU_TIMEOUT:-30}"

run_case() {
    name="$1"
    shift
    echo ""
    echo "==> ${name}"
    "${SCRIPT_DIR}/run-qemu.sh" --mode "$MODE" --smoke-timeout "$TIMEOUT" "$@"
}

"${SCRIPT_DIR}/build.sh" "$MODE"

run_case "nvme 4K split exFAT smoke ISO" \
    --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso

run_case "usb 512 split FAT32 smoke ISO" \
    --bus usb --layout split --data-fs fat32 --sector-size 512 --smoke-efi-iso

if [ "${NEXTBOOT_FULL_QEMU_MATRIX:-0}" = "1" ]; then
    run_case "virtio 512 single FAT32 smoke ISO" \
        --bus virtio --sector-size 512 --smoke-efi-iso

    run_case "usb 4K split exFAT smoke ISO" \
        --bus usb --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso

    run_case "sata 512 split NTFS smoke ISO" \
        --bus sata --layout split --data-fs ntfs --sector-size 512 --smoke-efi-iso

    run_case "nvme 4K split NTFS smoke ISO" \
        --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-efi-iso

    run_case "nvme 4K split UDF smoke ISO" \
        --bus nvme --layout split --data-fs udf --sector-size 4096 --smoke-efi-iso

    run_case "nvme 4K split ext2 smoke ISO" \
        --bus nvme --layout split --data-fs ext2 --sector-size 4096 --smoke-efi-iso

    run_case "nvme 4K split ext3 smoke ISO" \
        --bus nvme --layout split --data-fs ext3 --sector-size 4096 --smoke-efi-iso

    run_case "nvme 4K split XFS smoke ISO" \
        --bus nvme --layout split --data-fs xfs --sector-size 4096 --smoke-efi-iso

    run_case "nvme 512 split XFS smoke ISO" \
        --bus nvme --layout split --data-fs xfs --sector-size 512 --smoke-efi-iso

    run_case "nvme 4K split XFS VLNK smoke ISO" \
        --bus nvme --layout split --data-fs xfs --sector-size 4096 --smoke-vlnk-iso

    run_case "nvme 4K split exFAT raw IMG smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-raw-img

    run_case "nvme 4K split exFAT fixed VHD smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vhd

    run_case "nvme 4K split exFAT dynamic VHD smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-dynamic-vhd

    run_case "nvme 4K split exFAT VHDX smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vhdx

    run_case "nvme 4K split exFAT sparse VHDX smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-sparse-vhdx

    run_case "nvme 4K split exFAT partially-present VHDX smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-partial-vhdx

    run_case "nvme 4K split exFAT parent-required VHDX rejection" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-parent-vhdx

    run_case "nvme 4K split exFAT dynamic VDI smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vdi

    run_case "nvme 4K split exFAT static VDI smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-static-vdi

    run_case "nvme 4K split exFAT sparse VDI smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-sparse-vdi

    run_case "nvme 4K split exFAT discarded VDI smoke" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-discarded-vdi

    run_case "nvme 4K split exFAT parent-required VDI rejection" \
        --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-parent-vdi

    run_case "nvme 4K split ext4 Linux plugins" \
        --bus nvme --layout split --data-fs ext4 --sector-size 4096 --smoke-linux-plugins
fi

run_case "sd 512 split FAT32 image verification" \
    --bus sd --layout split --data-fs fat32 --sector-size 512 --smoke-efi-iso --no-run

if [ "${NEXTBOOT_QEMU_SD_BOOT_SMOKE:-0}" = "1" ]; then
    run_case "sd 512 split FAT32 experimental boot smoke" \
        --bus sd --layout split --data-fs fat32 --sector-size 512 --smoke-efi-iso
fi
