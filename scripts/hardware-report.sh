#!/usr/bin/env bash
# Collect a structured real-hardware compatibility report for NextBoot.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
HOST_OS="${NEXTBOOT_OSTYPE:-$OSTYPE}"

DEVICE=""
MEDIA="unknown"
BUS="unknown"
DATA_FS="unknown"
SECTOR_SIZE="unknown"
LAYOUT="split"
IMAGE_TYPE="unknown"
FIRMWARE="unknown"
RESULT="unknown"
NOTES=""
OUTPUT=""
APPEND_CSV=""
SMOKE_LOG=""

usage() {
    cat <<USAGE
NextBoot Hardware Report

Usage:
  $0 [options]

Options:
  --device TEXT       Device name or path tested, for example /dev/disk4
  --media TYPE        fixed, nvme, sata, usb, sd, enclosure, or other
  --bus BUS           UEFI/QEMU-style bus label: nvme, sata, usb, sd, virtio, other
  --data-fs FS        Data partition filesystem tested
  --sector-size BYTES Device logical sector size, usually 512 or 4096
  --layout LAYOUT     single or split (default: split)
  --image-type TYPE   iso, vlnk, img, vhd, vhdx, vdi, wim, or mixed
  --firmware TEXT     Machine or firmware name/version
  --result RESULT     pass, fail, partial, blocked, or unknown
  --notes TEXT        Short free-form note
  --smoke-log PATH    Include the tail of a captured QEMU/serial/console log
  --output PATH       Markdown report path (default: target/hardware-reports/*.md)
  --append-csv PATH   Append one machine-readable row to a CSV matrix
  -h, --help          Show this help

Example:
  $0 --device /dev/disk4 --media usb --bus usb --data-fs exfat \\
     --sector-size 512 --image-type iso --result pass \\
     --append-csv docs/hardware/hardware-matrix.csv
USAGE
}

die() {
    echo "Error: $*" >&2
    exit 1
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --device)
            [ $# -ge 2 ] || die "--device requires a value"
            DEVICE="$2"
            shift 2
            ;;
        --media)
            [ $# -ge 2 ] || die "--media requires a value"
            MEDIA="$2"
            shift 2
            ;;
        --bus)
            [ $# -ge 2 ] || die "--bus requires a value"
            BUS="$2"
            shift 2
            ;;
        --data-fs)
            [ $# -ge 2 ] || die "--data-fs requires a value"
            DATA_FS="$2"
            shift 2
            ;;
        --sector-size)
            [ $# -ge 2 ] || die "--sector-size requires a value"
            SECTOR_SIZE="$2"
            shift 2
            ;;
        --layout)
            [ $# -ge 2 ] || die "--layout requires a value"
            LAYOUT="$2"
            shift 2
            ;;
        --image-type)
            [ $# -ge 2 ] || die "--image-type requires a value"
            IMAGE_TYPE="$2"
            shift 2
            ;;
        --firmware)
            [ $# -ge 2 ] || die "--firmware requires a value"
            FIRMWARE="$2"
            shift 2
            ;;
        --result)
            [ $# -ge 2 ] || die "--result requires a value"
            RESULT="$2"
            shift 2
            ;;
        --notes)
            [ $# -ge 2 ] || die "--notes requires a value"
            NOTES="$2"
            shift 2
            ;;
        --smoke-log)
            [ $# -ge 2 ] || die "--smoke-log requires a value"
            SMOKE_LOG="$2"
            shift 2
            ;;
        --output)
            [ $# -ge 2 ] || die "--output requires a value"
            OUTPUT="$2"
            shift 2
            ;;
        --append-csv)
            [ $# -ge 2 ] || die "--append-csv requires a value"
            APPEND_CSV="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "Unknown argument: $1"
            ;;
    esac
done

case "$RESULT" in
    pass|fail|partial|blocked|unknown) ;;
    *) die "--result must be pass, fail, partial, blocked, or unknown" ;;
esac

timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
stamp_for_file="$(date -u +"%Y%m%dT%H%M%SZ")"
commit="$(git -C "$PROJECT_DIR" rev-parse --short HEAD 2>/dev/null || printf unknown)"
branch="$(git -C "$PROJECT_DIR" branch --show-current 2>/dev/null || printf unknown)"
host_uname="$(uname -a 2>/dev/null || printf unknown)"
arch="$(uname -m 2>/dev/null || printf unknown)"

if [ -z "$OUTPUT" ]; then
    OUTPUT="${PROJECT_DIR}/target/hardware-reports/${stamp_for_file}-${MEDIA}-${BUS}-${RESULT}.md"
fi
mkdir -p "$(dirname "$OUTPUT")"

tool_version() {
    if command_exists "$1"; then
        if command_exists rustup && [ "$1" = "rustc" -o "$1" = "cargo" ]; then
            if rustup toolchain list 2>/dev/null | grep -q '^1\.96\.0'; then
                rustup run 1.96.0 "$1" --version 2>&1 | head -n 1
                return
            fi
        fi
        "$1" --version 2>&1 | head -n 1
    else
        printf "not found"
    fi
}

collect_storage_inventory() {
    if [[ "$HOST_OS" == darwin* ]] && command_exists diskutil; then
        diskutil list
        return
    fi
    if command_exists lsblk; then
        lsblk -o NAME,SIZE,TYPE,TRAN,MODEL,VENDOR,LOG-SEC,PHY-SEC,MOUNTPOINT
        return
    fi
    printf "No disk inventory command found on this host.\n"
}

write_report() {
    {
        printf "# NextBoot Hardware Compatibility Report\n\n"
        printf "## Summary\n\n"
        printf "| Field | Value |\n"
        printf "| --- | --- |\n"
        printf "| Timestamp UTC | %s |\n" "$timestamp"
        printf "| Git commit | %s |\n" "$commit"
        printf "| Git branch | %s |\n" "$branch"
        printf "| Result | %s |\n" "$RESULT"
        printf "| Device | %s |\n" "${DEVICE:-unknown}"
        printf "| Media | %s |\n" "$MEDIA"
        printf "| Bus | %s |\n" "$BUS"
        printf "| Layout | %s |\n" "$LAYOUT"
        printf "| Data filesystem | %s |\n" "$DATA_FS"
        printf "| Sector size | %s |\n" "$SECTOR_SIZE"
        printf "| Image type | %s |\n" "$IMAGE_TYPE"
        printf "| Firmware | %s |\n" "$FIRMWARE"
        printf "| Notes | %s |\n\n" "${NOTES:-none}"

        printf "## Host\n\n"
        printf '```text\n'
        printf "%s\n" "$host_uname"
        printf "arch=%s\n" "$arch"
        printf "qemu=%s\n" "$(tool_version qemu-system-x86_64)"
        printf "rustc=%s\n" "$(tool_version rustc)"
        printf "cargo=%s\n" "$(tool_version cargo)"
        printf '```\n\n'

        printf "## Storage Inventory\n\n"
        printf '```text\n'
        collect_storage_inventory
        printf '```\n\n'

        printf "## Test Checklist\n\n"
        printf '%s\n' "- [ ] Flashed with scripts/flash.sh or equivalent split ESP/Data layout"
        printf '%s\n' "- [ ] Firmware saw the device as intended media type"
        printf '%s\n' "- [ ] NextBoot scanned Data partition and listed expected image"
        printf '%s\n' "- [ ] Selected image booted to its smoke marker or real OS handoff"
        printf '%s\n\n' "- [ ] Rebooted once to check firmware persistence/path stability"

        if [ -n "$SMOKE_LOG" ]; then
            printf "## Log Tail\n\n"
            printf '```text\n'
            tail -n 120 "$SMOKE_LOG"
            printf '```\n\n'
        fi
    } >"$OUTPUT"
}

csv_escape() {
    value="${1//\"/\"\"}"
    printf '"%s"' "$value"
}

append_csv() {
    [ -n "$APPEND_CSV" ] || return 0
    mkdir -p "$(dirname "$APPEND_CSV")"
    if [ ! -f "$APPEND_CSV" ]; then
        printf "timestamp,commit,branch,host_arch,device,media,bus,layout,data_fs,sector_size,image_type,firmware,result,report,notes\n" >"$APPEND_CSV"
    fi
    {
        csv_escape "$timestamp"; printf ","
        csv_escape "$commit"; printf ","
        csv_escape "$branch"; printf ","
        csv_escape "$arch"; printf ","
        csv_escape "${DEVICE:-unknown}"; printf ","
        csv_escape "$MEDIA"; printf ","
        csv_escape "$BUS"; printf ","
        csv_escape "$LAYOUT"; printf ","
        csv_escape "$DATA_FS"; printf ","
        csv_escape "$SECTOR_SIZE"; printf ","
        csv_escape "$IMAGE_TYPE"; printf ","
        csv_escape "$FIRMWARE"; printf ","
        csv_escape "$RESULT"; printf ","
        csv_escape "$OUTPUT"; printf ","
        csv_escape "${NOTES:-}"
        printf "\n"
    } >>"$APPEND_CSV"
}

write_report
append_csv
printf "Wrote %s\n" "$OUTPUT"
if [ -n "$APPEND_CSV" ]; then
    printf "Updated %s\n" "$APPEND_CSV"
fi
