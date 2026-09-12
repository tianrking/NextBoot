#!/usr/bin/env bash
# Update loaders and pinned runtime on existing media, preserving user files.

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
LOADERS_ONLY=0
ALLOW_LOOP=0
ROLLBACK=""
RUNTIME_DIR="${PROJECT_DIR}/target/runtime-assets"
MOUNT_ROOT=""
ESP_MOUNT=""
DATA_MOUNT=""
OWN_ESP_MOUNT=0
OWN_DATA_MOUNT=0

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
  --loaders-only    Explicitly omit DATA runtime migration
  --runtime-dir DIR Cached pinned runtime directory (default: target/runtime-assets)
  --rollback ID     Restore backups; use 'pending' for an interrupted update
  --allow-loop      Allow whole Linux loop disks for explicit image testing
  --dry-run         Inspect the actual disk and print commands without writing
  -y, --yes         Skip confirmation prompt
  -h, --help        Show this help

This updates ESP fallback loaders and the pinned runtime on NEXTDATA. Original
files are backed up and errors trigger rollback. /ISO and user configuration
are preserved. It never partitions or formats the disk.
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
            --loaders-only)
                LOADERS_ONLY=1
                shift
                ;;
            --allow-loop)
                ALLOW_LOOP=1
                shift
                ;;
            --runtime-dir|--rollback)
                [ "$#" -ge 2 ] || die "$1 requires a value"
                if [ "$1" = "--runtime-dir" ]; then RUNTIME_DIR="$2"; else ROLLBACK="$2"; fi
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
    [ "$ALLOW_LOOP" -eq 0 ] || args+=(--allow-loop)
    inventory="$("${PYTHON:-python3}" "$SCRIPT_DIR/media_partitions.py" --host "$host_kind" "${args[@]}" "$DEVICE")" || die "Partition inspection failed; nothing was written."
    IFS=$'\t' read -r ESP_PARTITION DATA_PARTITION <<< "$inventory"
    [ -n "$ESP_PARTITION" ] && [ -n "$DATA_PARTITION" ] || die "Incomplete partition inventory"
}

confirm_update() {
    if [ "$ASSUME_YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
        return
    fi
    warn "This updates bootloader/runtime files on ${DEVICE}, retaining backups."
    warn "/ISO and user configuration will be preserved; no partition is formatted."
    printf 'Type UPDATE to continue: '
    read -r answer
    [ "$answer" = "UPDATE" ] || die "aborted"
}

cleanup_mounts() {
    local cleanup_status=0
    if [[ "$HOST_OS" == "darwin"* ]]; then
        [ "$OWN_DATA_MOUNT" -eq 0 ] || run_cmd diskutil unmount "$DATA_PARTITION" || cleanup_status=1
        [ "$OWN_ESP_MOUNT" -eq 0 ] || run_cmd diskutil unmount "$ESP_PARTITION" || cleanup_status=1
    else
        [ "$OWN_DATA_MOUNT" -eq 0 ] || run_sudo umount "$DATA_MOUNT" || cleanup_status=1
        [ "$OWN_ESP_MOUNT" -eq 0 ] || run_sudo umount "$ESP_MOUNT" || cleanup_status=1
        if [ -n "$MOUNT_ROOT" ] && [ "$cleanup_status" -eq 0 ]; then
            # Only remove empty mount directories created for this invocation.
            [ -z "$DATA_MOUNT" ] || run_cmd rmdir "$DATA_MOUNT"
            run_cmd rmdir "$ESP_MOUNT" "$MOUNT_ROOT"
        fi
    fi
    return "$cleanup_status"
}

updater_macos_mountpoint() {
    diskutil info -plist "$1" | "${PYTHON:-python3}" -c \
        'import plistlib,sys; print(plistlib.load(sys.stdin.buffer).get("MountPoint", ""))'
}

mount_volumes() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            ESP_MOUNT="/Volumes/NEXBOOT"
            run_cmd diskutil mount "$ESP_PARTITION"
            if [ "$LOADERS_ONLY" -eq 0 ]; then
                DATA_MOUNT="/Volumes/NEXTDATA"
                run_cmd diskutil mount "$DATA_PARTITION"
            fi
            return
        fi
        ESP_MOUNT="$(updater_macos_mountpoint "$ESP_PARTITION")"
        if [ -z "$ESP_MOUNT" ] || [ "$ESP_MOUNT" = "Not mounted" ]; then
            run_cmd diskutil mount "$ESP_PARTITION"
            OWN_ESP_MOUNT=1
            ESP_MOUNT="$(updater_macos_mountpoint "$ESP_PARTITION")"
        fi
        [ -d "$ESP_MOUNT" ] || die "ESP mount could not be verified"
        if [ "$LOADERS_ONLY" -eq 0 ]; then
            DATA_MOUNT="$(updater_macos_mountpoint "$DATA_PARTITION")"
            if [ -z "$DATA_MOUNT" ] || [ "$DATA_MOUNT" = "Not mounted" ]; then
                run_cmd diskutil mount "$DATA_PARTITION"
                OWN_DATA_MOUNT=1
                DATA_MOUNT="$(updater_macos_mountpoint "$DATA_PARTITION")"
            fi
            [ -d "$DATA_MOUNT" ] || die "DATA mount could not be verified"
        fi
    else
        if [ "$DRY_RUN" -eq 1 ]; then MOUNT_ROOT="/tmp/nextboot-update.DRYRUN"; else
            MOUNT_ROOT="$(mktemp -d /tmp/nextboot-update.XXXXXX)"
        fi
        ESP_MOUNT="${MOUNT_ROOT}/esp"
        run_cmd mkdir -p "$ESP_MOUNT"
        run_sudo mount "$ESP_PARTITION" "$ESP_MOUNT"
        OWN_ESP_MOUNT=1
        if [ "$LOADERS_ONLY" -eq 0 ]; then
            DATA_MOUNT="${MOUNT_ROOT}/data"
            run_cmd mkdir -p "$DATA_MOUNT"
            run_sudo mount "$DATA_PARTITION" "$DATA_MOUNT"
            OWN_DATA_MOUNT=1
        fi
    fi
}

update_files() {
    local -a args=(--esp "$ESP_MOUNT")
    [ -z "$DATA_MOUNT" ] || args+=(--data "$DATA_MOUNT")
    [ "$LOADERS_ONLY" -eq 0 ] || args+=(--loaders-only)
    if [ -n "$ROLLBACK" ]; then args+=(--rollback "$ROLLBACK"); else
        args+=(--runtime-dir "$RUNTIME_DIR")
        for index in "${!EFI_INSTALL_FILES[@]}"; do
            args+=(--loader "${EFI_INSTALL_NAMES[$index]}=${EFI_INSTALL_FILES[$index]}")
        done
    fi
    if [[ "$HOST_OS" == "darwin"* ]]; then
        run_cmd "${PYTHON:-python3}" "$SCRIPT_DIR/update-mounted-media.py" "${args[@]}"
    else
        run_sudo "${PYTHON:-python3}" "$SCRIPT_DIR/update-mounted-media.py" "${args[@]}"
    fi
}

parse_args "$@"
normalize_device
if [ -z "$ROLLBACK" ]; then
    configure_flash_target
    resolve_efi_files
fi

if [ "$DRY_RUN" -eq 0 ] && [ ! -e "$DEVICE" ]; then
    die "Device not found: ${DEVICE}"
fi

identify_partitions
[ "$LOADERS_ONLY" -eq 1 ] || [ "$DATA_PARTITION" != "-" ] || die "Runtime update/recovery requires NEXTDATA; --force cannot waive this."
if [ -z "$ROLLBACK" ] && [ "$LOADERS_ONLY" -eq 0 ]; then
    run_cmd "${PYTHON:-python3}" "$SCRIPT_DIR/prepare-runtime-assets.py" --output "$RUNTIME_DIR"
fi

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
trap 'saved_status=$?; trap - EXIT; cleanup_mounts || saved_status=1; exit "$saved_status"' EXIT
mount_volumes
update_files
cleanup_mounts || die "Files updated, but a volume could not be unmounted safely."
OWN_ESP_MOUNT=0
OWN_DATA_MOUNT=0
MOUNT_ROOT=""
trap - EXIT

if [ "$DRY_RUN" -eq 1 ]; then
    info "Dry run complete. /ISO and user configuration would be preserved."
else
    info "Operation complete. /ISO and user configuration were preserved; backups retained."
fi
