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
ESP_PARTITION=""
DATA_PARTITION=""
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
  --force           Allow missing NEXTDATA label; still require a verified NextBoot ESP
  --dry-run         Inspect the actual disk and print commands without writing
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
    printf '%s\n' "$ESP_PARTITION"
}

data_partition() {
    printf '%s\n' "$DATA_PARTITION"
}

identify_partitions() {
    local host_kind inventory
    local -a args=()
    case "$HOST_OS" in
        darwin*) host_kind=darwin ;;
        linux*) host_kind=linux ;;
        *) die "This updater requires Linux or macOS; Windows support is not available yet." ;;
    esac
    [ "$FORCE" -eq 0 ] || args+=(--force)
    inventory="$("${PYTHON:-python3}" "$SCRIPT_DIR/media_partitions.py" --host "$host_kind" "${args[@]}" "$DEVICE")" || die "Partition inspection failed; nothing was written."
    IFS=$'\t' read -r ESP_PARTITION DATA_PARTITION <<< "$inventory"
    [ -n "$ESP_PARTITION" ] && [ -n "$DATA_PARTITION" ] || die "Incomplete partition inventory"
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

identify_partitions

info "NextBoot Media Updater"
warn "Target device: ${DEVICE}"
note "Verified ESP: ${ESP_PARTITION}; NEXTDATA: ${DATA_PARTITION}"
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
