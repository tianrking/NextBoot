#!/usr/bin/env bash
# NextBoot Flash Script
#
# Creates a standard GPT disk for NextBoot.  The default split layout keeps the
# UEFI bootloader on a small FAT32 ESP and stores images on a separate data
# partition, which matches fixed-disk SSD/NVMe deployments better than a single
# all-purpose FAT32 volume.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
HOST_OS="${NEXTBOOT_OSTYPE:-$OSTYPE}"

LAYOUT="split"
DATA_FS="exfat"
ESP_SIZE_MB=260
DRY_RUN=0
ASSUME_YES=0

usage() {
    cat <<USAGE
NextBoot Flash Tool

Usage:
  $0 list
  $0 [options] <device>

Options:
  --layout LAYOUT   Disk layout: split or single (default: split)
  --data-fs FS      Data partition filesystem for split layout: exfat or fat32 (default: exfat)
  --esp-size MB     ESP size for split layout in MiB (default: 260)
  --dry-run         Print the commands without writing to the device
  -y, --yes         Skip the confirmation prompt
  -h, --help        Show this help

Examples:
  $0 list
  $0 --layout split --data-fs exfat /dev/diskX
  $0 --layout split --data-fs fat32 /dev/sdX
  $0 --layout single /dev/sdX
USAGE
}

die() {
    echo -e "${RED}Error: $*${NC}" >&2
    exit 1
}

info() {
    echo -e "${GREEN}$*${NC}"
}

warn() {
    echo -e "${YELLOW}$*${NC}"
}

note() {
    echo -e "${BLUE}$*${NC}"
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

print_command() {
    printf '+'
    for arg in "$@"; do
        printf ' %q' "$arg"
    done
    printf '\n'
}

run_cmd() {
    print_command "$@"
    if [ "$DRY_RUN" -eq 0 ]; then
        "$@"
    fi
}

run_sudo() {
    run_cmd sudo "$@"
}

list_devices() {
    note "Available storage devices:"
    echo ""

    if [[ "$HOST_OS" == "darwin"* ]]; then
        diskutil list external | grep -E "^/dev/disk" || echo "No external drives found"
    else
        lsblk -o NAME,SIZE,TYPE,MODEL,MOUNTPOINT -d | grep -E "disk|sd|usb|nvme|mmcblk" || echo "No drives found"
    fi

    echo ""
    warn "Usage: $0 --layout split /dev/diskX"
}

normalize_macos_device() {
    case "$1" in
        /dev/rdisk*) printf '/dev/disk%s\n' "${1#/dev/rdisk}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

linux_partition_path() {
    case "$1" in
        *[0-9]) printf '%sp%s\n' "$1" "$2" ;;
        *) printf '%s%s\n' "$1" "$2" ;;
    esac
}

find_linux_exfat_mkfs() {
    if command_exists mkfs.exfat; then
        printf 'mkfs.exfat\n'
    elif command_exists mkexfatfs; then
        printf 'mkexfatfs\n'
    else
        return 1
    fi
}

require_linux_tools() {
    command_exists parted || die "parted is required"
    command_exists mkfs.vfat || die "mkfs.vfat is required"
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "exfat" ]; then
        find_linux_exfat_mkfs >/dev/null || die "mkfs.exfat or mkexfatfs is required for --data-fs exfat"
    fi
}

mount_point_for_macos_partition() {
    diskutil info "$1" | awk -F': *' '/Mount Point/ {print $2; exit}'
}

ensure_macos_mounted() {
    local partition="$1"
    local mount_point

    mount_point="$(mount_point_for_macos_partition "$partition")"
    if [ -z "$mount_point" ] || [ "$mount_point" = "Not mounted" ]; then
        run_cmd diskutil mount "$partition"
        mount_point="$(mount_point_for_macos_partition "$partition")"
    fi

    [ -n "$mount_point" ] && [ "$mount_point" != "Not mounted" ] || die "Could not mount ${partition}"
    printf '%s\n' "$mount_point"
}

copy_efi_tree() {
    local mount_point="$1"
    run_cmd mkdir -p "${mount_point}/EFI/BOOT"
    run_cmd cp "$EFI_FILE" "${mount_point}/EFI/BOOT/BOOTX64.EFI"
}

copy_efi_tree_sudo() {
    local mount_point="$1"
    run_sudo mkdir -p "${mount_point}/EFI/BOOT"
    run_sudo cp "$EFI_FILE" "${mount_point}/EFI/BOOT/BOOTX64.EFI"
}

parse_args() {
    if [ $# -eq 0 ]; then
        list_devices
        exit 0
    fi

    if [ "$1" = "list" ]; then
        list_devices
        exit 0
    fi

    DEVICE=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --layout)
                [ $# -ge 2 ] || die "--layout requires a value"
                LAYOUT="$2"
                shift 2
                ;;
            --data-fs)
                [ $# -ge 2 ] || die "--data-fs requires a value"
                DATA_FS="$2"
                shift 2
                ;;
            --esp-size)
                [ $# -ge 2 ] || die "--esp-size requires a value"
                ESP_SIZE_MB="$2"
                shift 2
                ;;
            --dry-run)
                DRY_RUN=1
                shift
                ;;
            -y|--yes)
                ASSUME_YES=1
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                die "Unknown option: $1"
                ;;
            *)
                if [ -n "${DEVICE}" ]; then
                    die "Only one target device can be specified"
                fi
                DEVICE="$1"
                shift
                ;;
        esac
    done

    [ -n "$DEVICE" ] || die "Missing target device"
}

parse_args "$@"

case "$LAYOUT" in
    split|single) ;;
    *) die "--layout must be split or single" ;;
esac

case "$DATA_FS" in
    exfat|fat32) ;;
    *) die "--data-fs must be exfat or fat32" ;;
esac

case "$ESP_SIZE_MB" in
    ''|*[!0-9]*) die "--esp-size must be an integer MiB value" ;;
esac

if [ "$ESP_SIZE_MB" -lt 64 ]; then
    die "--esp-size must be at least 64 MiB"
fi

EFI_FILE="${PROJECT_DIR}/target/x86_64-unknown-uefi/release/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    EFI_FILE="${PROJECT_DIR}/target/x86_64-unknown-uefi/debug/nextboot-boot.efi"
fi

[ -f "$EFI_FILE" ] || die "EFI file not found. Run ./scripts/build.sh first."

if [ "$DRY_RUN" -eq 0 ] && [ ! -e "$DEVICE" ]; then
    die "Device not found: ${DEVICE}"
fi

echo -e "${GREEN}NextBoot Flash Tool${NC}"
echo "===================="
warn "EFI file: ${EFI_FILE}"
warn "Target device: ${DEVICE}"
warn "Layout: ${LAYOUT}"
if [ "$LAYOUT" = "split" ]; then
    warn "ESP size: ${ESP_SIZE_MB} MiB"
    warn "Data filesystem: ${DATA_FS}"
fi
if [ "$DRY_RUN" -eq 1 ]; then
    note "Dry run: no commands will be executed"
fi
echo ""

if [ "$ASSUME_YES" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
    echo -e "${RED}WARNING: This will ERASE ALL DATA on ${DEVICE}${NC}"
    echo -n "Are you sure? (yes/no): "
    read -r CONFIRM
    [ "$CONFIRM" = "yes" ] || { echo "Aborted"; exit 0; }
fi

warn "Unmounting device..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    DEVICE="$(normalize_macos_device "$DEVICE")"
    run_cmd diskutil unmountDisk "$DEVICE" || true
else
    if [ "$DRY_RUN" -eq 0 ]; then
        require_linux_tools
    fi
    run_sudo umount "${DEVICE}"* || true
fi

warn "Creating GPT partition table..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    if [ "$LAYOUT" = "split" ]; then
        if [ "$DATA_FS" = "fat32" ]; then
            MAC_DATA_FS="FAT32"
        else
            MAC_DATA_FS="ExFAT"
        fi
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT "${ESP_SIZE_MB}MiB" "$MAC_DATA_FS" NEXTDATA R
    else
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT 100%
    fi
else
    run_sudo parted -s "$DEVICE" mklabel gpt
    if [ "$LAYOUT" = "split" ]; then
        esp_end="${ESP_SIZE_MB}MiB"
        run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB "$esp_end"
        run_sudo parted -s "$DEVICE" set 1 esp on
        run_sudo parted -s "$DEVICE" mkpart NEXTDATA fat32 "$esp_end" 100%
    else
        run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB 100%
        run_sudo parted -s "$DEVICE" set 1 esp on
    fi
    run_sudo partprobe "$DEVICE" || true
    if [ "$DRY_RUN" -eq 0 ]; then
        sleep 2
    fi

    ESP_PART="$(linux_partition_path "$DEVICE" 1)"
    run_sudo mkfs.vfat -F 32 -n NEXBOOT "$ESP_PART"
    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="$(linux_partition_path "$DEVICE" 2)"
        if [ "$DATA_FS" = "exfat" ]; then
            if [ "$DRY_RUN" -eq 1 ]; then
                EXFAT_MKFS="mkfs.exfat"
            else
                EXFAT_MKFS="$(find_linux_exfat_mkfs)"
            fi
            run_sudo "$EXFAT_MKFS" -n NEXTDATA "$DATA_PART"
        else
            run_sudo mkfs.vfat -F 32 -n NEXTDATA "$DATA_PART"
        fi
    fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
    echo ""
    info "Dry run complete. No data was written."
    exit 0
fi

warn "Copying files..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    ESP_PART="${DEVICE}s1"
    ESP_MOUNT="$(ensure_macos_mounted "$ESP_PART")"
    copy_efi_tree "$ESP_MOUNT"

    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="${DEVICE}s2"
        DATA_MOUNT="$(ensure_macos_mounted "$DATA_PART")"
        run_cmd mkdir -p "${DATA_MOUNT}/ISO"
        sync
        run_cmd diskutil unmount "$DATA_PART"
    else
        run_cmd mkdir -p "${ESP_MOUNT}/ISO"
        sync
    fi
    run_cmd diskutil unmount "$ESP_PART"
else
    ESP_PART="$(linux_partition_path "$DEVICE" 1)"
    ESP_MOUNT="/tmp/nextboot_flash_esp"
    DATA_MOUNT="/tmp/nextboot_flash_data"

    run_sudo mkdir -p "$ESP_MOUNT"
    run_sudo mount "$ESP_PART" "$ESP_MOUNT"
    copy_efi_tree_sudo "$ESP_MOUNT"

    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="$(linux_partition_path "$DEVICE" 2)"
        run_sudo mkdir -p "$DATA_MOUNT"
        run_sudo mount "$DATA_PART" "$DATA_MOUNT"
        run_sudo mkdir -p "${DATA_MOUNT}/ISO"
        sync
        run_sudo umount "$DATA_MOUNT"
    else
        run_sudo mkdir -p "${ESP_MOUNT}/ISO"
        sync
    fi
    run_sudo umount "$ESP_MOUNT"
fi

echo ""
info "Flash complete!"
echo ""
if [ "$LAYOUT" = "split" ]; then
    echo "Copy ISO/WIM/VHD files to the Data partition's /ISO directory and boot from the device."
else
    echo "Copy ISO/WIM/VHD files to /ISO and boot from the device."
fi
