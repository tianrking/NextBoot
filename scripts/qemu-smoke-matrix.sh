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

    run_case "sata 512 split NTFS smoke ISO" \
        --bus sata --layout split --data-fs ntfs --sector-size 512 --smoke-efi-iso
fi

run_case "sd 512 split FAT32 image verification" \
    --bus sd --layout split --data-fs fat32 --sector-size 512 --smoke-efi-iso --no-run

if [ "${NEXTBOOT_QEMU_SD_BOOT_SMOKE:-0}" = "1" ]; then
    run_case "sd 512 split FAT32 experimental boot smoke" \
        --bus sd --layout split --data-fs fat32 --sector-size 512 --smoke-efi-iso
fi
