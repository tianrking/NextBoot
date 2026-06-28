#!/usr/bin/env bash
# One-command NextBoot release media writer for macOS and Linux.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOST_OS="${NEXTBOOT_OSTYPE:-$OSTYPE}"
IMAGE=""
DEVICE=""
SECTOR_SIZE=""
ASSUME_YES=0
DRY_RUN=0
ALLOW_FILE=0
NO_MOUNT=0
PYTHON_BIN=""

usage() {
    cat <<USAGE
NextBoot Burn Tool

Usage:
  $0 list
  $0 --image nextboot-...-universal-512b-exfat.img.xz [options] <device>

Options:
  --image PATH       NextBoot release .img or .img.xz to write
  --sector-size N    Image logical sector size: 512 or 4096 (auto from name)
  --allow-file       Permit a regular file target for tests
  --no-mount         Do not try to remount NEXTDATA after writing
  --dry-run          Print commands without writing
  -y, --yes          Skip confirmation prompt
  -h, --help         Show this help

Examples:
  $0 list
  $0 --image nextboot-v0.0.1-all-uefi-universal-512b-exfat.img.xz /dev/disk4
  $0 --image nextboot-v0.0.1-all-uefi-universal-512b-exfat.img.xz /dev/sdb

This writes the whole-disk release image, expands NEXTDATA to the target media,
and then mounts or refreshes the device when the host OS supports it.
USAGE
}

die() {
    echo "error: $*" >&2
    exit 1
}

warn() {
    printf '%s\n' "$*" >&2
}

print_command() {
    printf '+'
    for arg in "$@"; do
        printf ' %q' "$arg"
    done
    printf '\n'
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

run_cmd() {
    print_command "$@"
    if [ "$DRY_RUN" -eq 0 ]; then
        "$@"
    fi
}

run_root() {
    if [ "$ALLOW_FILE" -eq 1 ]; then
        run_cmd "$@"
    else
        run_cmd sudo "$@"
    fi
}

list_devices() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        diskutil list external
    else
        lsblk -o NAME,SIZE,TYPE,MODEL,TRAN,MOUNTPOINT -d
    fi
}

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
            --image)
                [ "$#" -ge 2 ] || die "--image requires a path"
                IMAGE="$2"
                shift 2
                ;;
            --sector-size)
                [ "$#" -ge 2 ] || die "--sector-size requires a value"
                SECTOR_SIZE="$2"
                shift 2
                ;;
            --allow-file)
                ALLOW_FILE=1
                shift
                ;;
            --no-mount)
                NO_MOUNT=1
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
                die "unknown option: $1"
                ;;
            *)
                [ -z "$DEVICE" ] || die "only one target device can be specified"
                DEVICE="$1"
                shift
                ;;
        esac
    done
    [ -n "$IMAGE" ] || die "--image is required"
    [ -n "$DEVICE" ] || die "target device is required"
}

infer_sector_size() {
    if [ -n "$SECTOR_SIZE" ]; then
        return
    fi
    case "$(basename "$IMAGE")" in
        *-4096b-*) SECTOR_SIZE=4096 ;;
        *-512b-*) SECTOR_SIZE=512 ;;
        *) SECTOR_SIZE=512 ;;
    esac
}

normalize_device() {
    if [[ "$HOST_OS" == "darwin"* ]]; then
        case "$DEVICE" in
            /dev/rdisk*) DEVICE="/dev/disk${DEVICE#/dev/rdisk}" ;;
        esac
    fi
}

raw_write_device() {
    if [[ "$HOST_OS" == "darwin"* ]] && [ "$ALLOW_FILE" -eq 0 ]; then
        printf '/dev/rdisk%s\n' "${DEVICE#/dev/disk}"
    else
        printf '%s\n' "$DEVICE"
    fi
}

stat_size() {
    if stat -f %z "$1" >/dev/null 2>&1; then
        stat -f %z "$1"
    else
        stat -c %s "$1"
    fi
}

diskutil_field() {
    diskutil info "$DEVICE" | sed -n "s/^[[:space:]]*$1:[[:space:]]*//p" | head -1
}

device_size_bytes() {
    if [ "$ALLOW_FILE" -eq 1 ]; then
        stat_size "$DEVICE"
    elif [[ "$HOST_OS" == "darwin"* ]]; then
        diskutil_field "Disk Size" | sed -n 's/.*(\([0-9][0-9]*\) Bytes).*/\1/p'
    else
        blockdev --getsize64 "$DEVICE"
    fi
}

device_block_size() {
    if [ "$ALLOW_FILE" -eq 1 ]; then
        printf '%s\n' "$SECTOR_SIZE"
    elif [[ "$HOST_OS" == "darwin"* ]]; then
        diskutil_field "Device Block Size" | sed -n 's/\([0-9][0-9]*\).*/\1/p'
    else
        blockdev --getss "$DEVICE"
    fi
}

validate_inputs() {
    [ -f "$IMAGE" ] || die "image not found: $IMAGE"
    case "$SECTOR_SIZE" in
        512|4096) ;;
        *) die "--sector-size must be 512 or 4096" ;;
    esac
    if [ "$ALLOW_FILE" -eq 0 ] && [ ! -e "$DEVICE" ]; then
        die "device not found: $DEVICE"
    fi
    PYTHON_BIN="$(command -v python3 || true)"
    [ -n "$PYTHON_BIN" ] || die "python3 is required"
    [ -x "$SCRIPT_DIR/grow-release-media.py" ] || die "missing grow-release-media.py"
}

guard_sector_size() {
    local block_size="$1"
    [ -n "$block_size" ] || die "could not determine target block size"
    if [ "$block_size" != "$SECTOR_SIZE" ] && [ "$ALLOW_FILE" -eq 0 ]; then
        die "target reports ${block_size}B sectors but image is ${SECTOR_SIZE}B; use a matching NextBoot image"
    fi
}

unmount_device() {
    if [ "$ALLOW_FILE" -eq 1 ]; then
        return
    fi
    if [[ "$HOST_OS" == "darwin"* ]]; then
        run_cmd diskutil unmountDisk "$DEVICE" || true
    else
        run_root umount "${DEVICE}"* || true
    fi
}

decompress_xz() {
    "$PYTHON_BIN" - "$IMAGE" <<'PY'
import lzma
import shutil
import sys

with lzma.open(sys.argv[1], "rb") as src:
    shutil.copyfileobj(src, sys.stdout.buffer, length=1024 * 1024)
PY
}

write_image() {
    local output
    local bs_arg="bs=4M"
    local conv_arg="conv=fsync,notrunc"
    local status_arg="status=progress"
    output="$(raw_write_device)"
    if [[ "$HOST_OS" == "darwin"* ]]; then
        bs_arg="bs=4m"
        conv_arg="conv=notrunc"
        status_arg=""
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        case "$IMAGE" in
            *.xz) print_command python3 lzma-decompress "$IMAGE" "|" dd "of=$output" "$bs_arg" "$conv_arg" "$status_arg" ;;
            *) print_command dd "if=$IMAGE" "of=$output" "$bs_arg" "$conv_arg" "$status_arg" ;;
        esac
        return
    fi

    case "$IMAGE" in
        *.xz)
            if [ "$ALLOW_FILE" -eq 1 ]; then
                decompress_xz | dd "of=$output" "$bs_arg" "$conv_arg"
            else
                decompress_xz | sudo dd "of=$output" "$bs_arg" "$conv_arg" ${status_arg:+"$status_arg"}
            fi
            ;;
        *)
            if [ -n "$status_arg" ]; then
                run_root dd "if=$IMAGE" "of=$output" "$bs_arg" "$conv_arg" "$status_arg"
            else
                run_root dd "if=$IMAGE" "of=$output" "$bs_arg" "$conv_arg"
            fi
            ;;
    esac
    sync
}

grow_target() {
    local size_bytes="$1"
    local output
    output="$(raw_write_device)"
    run_root "$PYTHON_BIN" "$SCRIPT_DIR/grow-release-media.py" \
        --disk-image "$output" \
        --sector-size "$SECTOR_SIZE" \
        --media-size-bytes "$size_bytes"
}

refresh_device() {
    if [ "$NO_MOUNT" -eq 1 ] || [ "$ALLOW_FILE" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
        return
    fi
    if [[ "$HOST_OS" == "darwin"* ]]; then
        run_cmd diskutil mountDisk "$DEVICE" || true
    else
        run_root partprobe "$DEVICE" || true
        command_exists udevadm && run_root udevadm settle || true
        if command_exists udisksctl; then
            udisksctl mount -b "$(linux_data_partition "$DEVICE")" || true
        fi
    fi
}

linux_data_partition() {
    case "$1" in
        *[0-9]) printf '%sp2\n' "$1" ;;
        *) printf '%s2\n' "$1" ;;
    esac
}

confirm() {
    if [ "$ASSUME_YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
        return
    fi
    warn "WARNING: this will erase all data on $DEVICE"
    printf 'Type YES to continue: '
    read -r answer
    [ "$answer" = "YES" ] || die "aborted"
}

parse_args "$@"
infer_sector_size
normalize_device
validate_inputs

media_size="$(device_size_bytes)"
target_block_size="$(device_block_size)"
[ -n "$media_size" ] || die "could not determine target size"
guard_sector_size "$target_block_size"

echo "NextBoot Burn Tool"
echo "Image: $IMAGE"
echo "Target: $DEVICE"
echo "Target size: $media_size bytes"
echo "Image sector size: $SECTOR_SIZE"
confirm

unmount_device
write_image
grow_target "$media_size"
refresh_device

echo "Done. Open NEXTDATA and copy ISO/WIM/VHD/VHDX/IMG/EFI files into /ISO."
