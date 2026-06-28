#!/usr/bin/env bash
# Update the NextBoot UEFI loaders on an existing disk without touching NEXTDATA.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
HOST_OS="${NEXTBOOT_OSTYPE:-$OSTYPE}"
TARGET="${TARGET:-all}"
DEVICE=""
DRY_RUN=0
ASSUME_YES=0
FORCE=0
EFI_INSTALL_FILES=()
EFI_INSTALL_NAMES=()
EFI_INSTALL_TARGETS=()

usage() {
    cat <<USAGE
NextBoot Media Updater

Usage:
  $0 list
  $0 [options] <device>

Options:
  --target TARGET   UEFI target to update: x86_64-unknown-uefi,
                    i686-unknown-uefi, aarch64-unknown-uefi, or all
                    (default: all)
  --force           Update even if NEXTDATA cannot be detected
  --dry-run         Print commands without writing
  -y, --yes         Skip confirmation prompt
  -h, --help        Show this help

This updates only the ESP fallback loaders under EFI/BOOT. It does not
partition, format, delete, or copy anything in the NEXTDATA /ISO partition.
USAGE
}

die() {
    echo -e "${RED}Error: $*${NC}" >&2
    exit 1
}

warn() {
    echo -e "${YELLOW}$*${NC}" >&2
}

note() {
    echo -e "${BLUE}$*${NC}"
}

info() {
    echo -e "${GREEN}$*${NC}"
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
}

source "${SCRIPT_DIR}/lib/flash_helpers.sh"
source "${SCRIPT_DIR}/lib/flash_targets.sh"

parse_args() {
    if [ "$#" -eq 0 ]; then
        usage
        exit 0
    fi
    if [ "$1" = "list" ]; then
        list_devices
        exit 0
    fi

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --target)
                [ "$#" -ge 2 ] || die "--target requires a value"
                TARGET="$2"
                shift 2
                ;;
            --force)
                FORCE=1
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
                [ -z "$DEVICE" ] || die "Only one target device can be specified"
                DEVICE="$1"
                shift
                ;;
        esac
    done

    [ -n "$DEVICE" ] || die "Missing target device"
}

normalize_device() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        DEVICE="$(normalize_macos_device "$DEVICE")"
    fi
}

esp_partition() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        printf '%ss1\n' "$DEVICE"
    else
        linux_partition_path "$DEVICE" 1
    fi
}

data_partition() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        printf '%ss2\n' "$DEVICE"
    else
        linux_partition_path "$DEVICE" 2
    fi
}

has_nextdata_partition() {
    if [ "$DRY_RUN" -eq 1 ]; then
        return 0
    fi

    local data_part
    data_part="$(data_partition)"
    if [[ "$HOST_OS" == "darwin"* ]]; then
        diskutil info "$data_part" 2>/dev/null | grep -Eq 'Volume Name:[[:space:]]+NEXTDATA'
    else
        [ -e "$data_part" ] || return 1
        lsblk -no LABEL "$data_part" 2>/dev/null | grep -qx 'NEXTDATA'
    fi
}

confirm_update() {
    if [ "$ASSUME_YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
        return
    fi
    warn "This updates only the ESP bootloader files on ${DEVICE}."
    warn "NEXTDATA and /ISO contents will not be formatted or deleted."
    printf 'Type UPDATE to continue: '
    read -r answer
    [ "$answer" = "UPDATE" ] || die "aborted"
}

update_macos_esp() {
    local esp_part
    local esp_mount
    esp_part="$(esp_partition)"
    if [ "$DRY_RUN" -eq 1 ]; then
        run_cmd diskutil mount "$esp_part"
        esp_mount="/Volumes/NEXBOOT"
    else
        esp_mount="$(ensure_macos_mounted "$esp_part")"
    fi
    copy_efi_tree "$esp_mount"
    sync
    run_cmd diskutil unmount "$esp_part"
}

update_linux_esp() {
    local esp_part
    local esp_mount="/tmp/nextboot_update_esp"
    esp_part="$(esp_partition)"
    run_sudo mkdir -p "$esp_mount"
    run_sudo mount "$esp_part" "$esp_mount"
    copy_efi_tree_sudo "$esp_mount"
    sync
    run_sudo umount "$esp_mount"
}

parse_args "$@"
normalize_device
configure_flash_target
resolve_efi_files

if [ "$DRY_RUN" -eq 0 ] && [ ! -e "$DEVICE" ]; then
    die "Device not found: ${DEVICE}"
fi

if ! has_nextdata_partition && [ "$FORCE" -eq 0 ]; then
    die "NEXTDATA was not detected on $(data_partition); use --force only if this is a NextBoot disk"
fi

info "NextBoot Media Updater"
warn "Target device: ${DEVICE}"
for index in "${!EFI_INSTALL_FILES[@]}"; do
    warn "Update ${EFI_INSTALL_TARGETS[$index]}: ${EFI_INSTALL_FILES[$index]} -> EFI/BOOT/${EFI_INSTALL_NAMES[$index]}"
done
if [ "$DRY_RUN" -eq 1 ]; then
    note "Dry run: no commands will be executed"
fi

confirm_update

if [[ "$HOST_OS" == "darwin"* ]]; then
    update_macos_esp
else
    update_linux_esp
fi

if [ "$DRY_RUN" -eq 1 ]; then
    info "Dry run complete. NEXTDATA would be preserved."
else
    info "Update complete. NEXTDATA and /ISO were preserved."
fi
