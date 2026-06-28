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
TARGET="${TARGET:-x86_64-unknown-uefi}"
EFI_INSTALL_FILES=()
EFI_INSTALL_NAMES=()

LAYOUT="split"
DATA_FS="exfat"
ESP_SIZE_MB=260
INSTALL_VENTOY_ASSETS=1
VENTOY_ASSETS_DIR=""
VENTOY_ASSETS_EXPLICIT=0
VENTOY_ASSETS_RESOLVED=""
DRY_RUN=0
ASSUME_YES=0

usage() {
    cat <<USAGE
NextBoot Flash Tool

Usage:
  $0 list
  $0 [options] <device>

Options:
  --target TARGET   UEFI build target: x86_64-unknown-uefi, i686-unknown-uefi,
                    aarch64-unknown-uefi, or all
  --layout LAYOUT   Disk layout: split or single (default: split)
  --data-fs FS      Data partition filesystem for split layout: exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs (default: exfat)
  --esp-size MB     ESP size for split layout in MiB (default: 260)
  --ventoy-assets DIR
                    Install WIMBOOT assets from DIR into /ventoy
  --no-ventoy-assets
                    Do not auto-install Ventoy WIMBOOT assets
  --dry-run         Print the commands without writing to the device
  -y, --yes         Skip the confirmation prompt
  -h, --help        Show this help

Examples:
  $0 list
  $0 --layout split --data-fs exfat /dev/diskX
  $0 --layout split --data-fs ext3 /dev/nvme0n1
  $0 --layout split --data-fs ext4 /dev/nvme0n1
  $0 --layout split --data-fs ntfs /dev/diskX
  $0 --layout split --data-fs udf /dev/sdX
  $0 --layout split --data-fs xfs /dev/nvme0n1
  $0 --target i686-unknown-uefi --layout split --data-fs exfat /dev/diskX
  $0 --target aarch64-unknown-uefi --layout split --data-fs exfat /dev/diskX
  $0 --target all --layout split --data-fs exfat /dev/diskX
  $0 --layout split --ventoy-assets ../Ventoy/INSTALL/ventoy /dev/diskX
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

source "${SCRIPT_DIR}/lib/flash_helpers.sh"
source "${SCRIPT_DIR}/lib/flash_partitioning.sh"
source "${SCRIPT_DIR}/lib/flash_population.sh"
source "${SCRIPT_DIR}/lib/flash_targets.sh"

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
            --target)
                [ $# -ge 2 ] || die "--target requires a value"
                TARGET="$2"
                shift 2
                ;;
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
            --ventoy-assets)
                [ $# -ge 2 ] || die "--ventoy-assets requires a directory"
                VENTOY_ASSETS_DIR="$2"
                VENTOY_ASSETS_EXPLICIT=1
                INSTALL_VENTOY_ASSETS=1
                shift 2
                ;;
            --no-ventoy-assets)
                INSTALL_VENTOY_ASSETS=0
                shift
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
configure_flash_target

case "$LAYOUT" in
    split|single) ;;
    *) die "--layout must be split or single" ;;
esac

case "$DATA_FS" in
    exfat|ext2|ext3|ext4|fat32|ntfs|udf|xfs) ;;
    *) die "--data-fs must be exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs" ;;
esac

case "$ESP_SIZE_MB" in
    ''|*[!0-9]*) die "--esp-size must be an integer MiB value" ;;
esac

if [ "$LAYOUT" = "single" ] && [ "$DATA_FS" != "exfat" ]; then
    warn "--data-fs is ignored for single layout"
fi

if [ "$ESP_SIZE_MB" -lt 64 ]; then
    die "--esp-size must be at least 64 MiB"
fi

resolve_efi_files

if [ "$INSTALL_VENTOY_ASSETS" -eq 1 ]; then
    if VENTOY_ASSETS_RESOLVED="$(resolve_ventoy_assets_dir)"; then
        :
    elif [ "$VENTOY_ASSETS_EXPLICIT" -eq 1 ]; then
        [ -d "$VENTOY_ASSETS_DIR" ] || die "Ventoy assets directory not found: ${VENTOY_ASSETS_DIR}"
        die "Missing ${VENTOY_ASSETS_DIR}/wimboot.x86_64.xz"
    else
        warn "Ventoy WIMBOOT assets were not found; Windows WIMBOOT fallback will need /ventoy/wimboot.x86_64.xz copied later."
    fi
fi

if [ "$DRY_RUN" -eq 0 ] && [ ! -e "$DEVICE" ]; then
    die "Device not found: ${DEVICE}"
fi

echo -e "${GREEN}NextBoot Flash Tool${NC}"
echo "===================="
warn "UEFI target: ${TARGET}"
for index in "${!EFI_INSTALL_FILES[@]}"; do
    warn "EFI ${EFI_INSTALL_TARGETS[$index]}: ${EFI_INSTALL_FILES[$index]} -> EFI/BOOT/${EFI_INSTALL_NAMES[$index]}"
done
warn "Target device: ${DEVICE}"
warn "Layout: ${LAYOUT}"
if [ "$LAYOUT" = "split" ]; then
    warn "ESP size: ${ESP_SIZE_MB} MiB"
    warn "Data filesystem: ${DATA_FS}"
fi
if [ -n "$VENTOY_ASSETS_RESOLVED" ]; then
    warn "Ventoy assets: ${VENTOY_ASSETS_RESOLVED}"
elif [ "$INSTALL_VENTOY_ASSETS" -eq 0 ]; then
    warn "Ventoy assets: disabled"
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

unmount_target_device
create_target_partitions

if [ "$DRY_RUN" -eq 1 ]; then
    echo ""
    info "Dry run complete. No data was written."
    exit 0
fi

populate_target_media

echo ""
info "Flash complete!"
echo ""
if [ "$LAYOUT" = "split" ]; then
    echo "Copy ISO/WIM/VHD files to the Data partition's /ISO directory and boot from the device."
else
    echo "Copy ISO/WIM/VHD files to /ISO and boot from the device."
fi
