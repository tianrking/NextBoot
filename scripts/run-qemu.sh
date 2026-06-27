#!/usr/bin/env bash
# NextBoot QEMU Test Script
#
# Creates a GPT-partitioned FAT32 disk image with NextBoot installed as the
# removable/fallback UEFI bootloader.  The image can be attached through several
# QEMU storage buses so fixed-disk, NVMe, SATA, USB, and virtio paths can be
# tested without rewriting real media.
#
# Usage:
#   ./scripts/run-qemu.sh
#   ./scripts/run-qemu.sh release
#   ./scripts/run-qemu.sh --bus nvme --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus usb --mode release --no-run

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="x86_64-unknown-uefi"
BUILD_MODE="debug"
BUS="virtio"
DISK_SIZE_MB=256
DISK_IMG=""
NO_RUN=0
MEMORY="1024M"
IMAGES=()

usage() {
    cat <<USAGE
NextBoot QEMU Test

Usage:
  $0 [debug|release] [options]

Options:
  --mode MODE        Build mode: debug or release
  --bus BUS          Storage bus: virtio, nvme, sata, usb
  --image PATH       Copy an ISO/WIM/VHD image into /ISO (repeatable)
  --disk-size MB     GPT disk image size in MiB (default: 256)
  --disk-image PATH  Output disk image path
  --memory SIZE      QEMU guest memory (default: 1024M)
  --no-run           Create the disk image and print the QEMU command only
  -h, --help         Show this help

Examples:
  $0 --bus nvme --image ~/Downloads/Win11.iso
  $0 release --bus sata --disk-size 4096
  $0 --bus usb --no-run
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

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "$2"
}

while [ $# -gt 0 ]; do
    case "$1" in
        debug|release)
            BUILD_MODE="$1"
            shift
            ;;
        --mode)
            [ $# -ge 2 ] || die "--mode requires a value"
            BUILD_MODE="$2"
            shift 2
            ;;
        --bus)
            [ $# -ge 2 ] || die "--bus requires a value"
            BUS="$2"
            shift 2
            ;;
        --image)
            [ $# -ge 2 ] || die "--image requires a path"
            IMAGES+=("$2")
            shift 2
            ;;
        --disk-size)
            [ $# -ge 2 ] || die "--disk-size requires a value"
            DISK_SIZE_MB="$2"
            shift 2
            ;;
        --disk-image)
            [ $# -ge 2 ] || die "--disk-image requires a path"
            DISK_IMG="$2"
            shift 2
            ;;
        --memory)
            [ $# -ge 2 ] || die "--memory requires a value"
            MEMORY="$2"
            shift 2
            ;;
        --no-run)
            NO_RUN=1
            shift
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

case "$BUILD_MODE" in
    debug|release) ;;
    *) die "Invalid build mode: ${BUILD_MODE}" ;;
esac

case "$BUS" in
    virtio|nvme|sata|usb) ;;
    *) die "Invalid bus '${BUS}'. Use virtio, nvme, sata, or usb." ;;
esac

case "$DISK_SIZE_MB" in
    ''|*[!0-9]*) die "--disk-size must be an integer MiB value" ;;
esac

if [ "$DISK_SIZE_MB" -lt 64 ]; then
    die "--disk-size must be at least 64 MiB"
fi

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    die "EFI file not found: ${EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
fi

for image in "${IMAGES[@]}"; do
    [ -f "$image" ] || die "Image file not found: ${image}"
done

if [ -z "$DISK_IMG" ]; then
    DISK_IMG="${PROJECT_DIR}/target/qemu_${BUS}_${BUILD_MODE}.img"
fi

mkdir -p "$(dirname "$DISK_IMG")"

echo -e "${GREEN}NextBoot QEMU Test${NC}"
echo "=================="
info "EFI file: ${EFI_FILE}"
info "Storage bus: ${BUS}"
info "Disk image: ${DISK_IMG}"

require_command python3 "python3 is required to create the GPT disk image"
require_command mformat "mtools is required: install with 'brew install mtools' or your distro package manager"
require_command mcopy "mtools is required: install with 'brew install mtools' or your distro package manager"
require_command mmd "mtools is required: install with 'brew install mtools' or your distro package manager"

SECTOR_SIZE=512
PART_START_LBA=2048
TOTAL_SECTORS=$((DISK_SIZE_MB * 1024 * 1024 / SECTOR_SIZE))
PART_SECTORS=$((TOTAL_SECTORS - PART_START_LBA - 33))
PART_OFFSET_BYTES=$((PART_START_LBA * SECTOR_SIZE))

if [ "$PART_SECTORS" -le 0 ]; then
    die "Disk image is too small for GPT and FAT32 partition"
fi

warn "Creating GPT/FAT32 test disk image..."
python3 - "$DISK_IMG" "$DISK_SIZE_MB" "$PART_START_LBA" <<'PY'
import os
import struct
import sys
import uuid
import zlib

path = sys.argv[1]
size_mb = int(sys.argv[2])
part_start_lba = int(sys.argv[3])
sector_size = 512
total_sectors = size_mb * 1024 * 1024 // sector_size
last_lba = total_sectors - 1
part_end_lba = last_lba - 33

if part_start_lba >= part_end_lba:
    raise SystemExit("disk image is too small")

disk_guid = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-qemu:{os.path.abspath(path)}").bytes_le
part_guid = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-qemu-part:{os.path.abspath(path)}").bytes_le
esp_type = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").bytes_le

with open(path, "wb") as f:
    f.truncate(total_sectors * sector_size)

    mbr = bytearray(sector_size)
    mbr[0x1BE] = 0x00
    mbr[0x1BE + 4] = 0xEE
    mbr[0x1BE + 8:0x1BE + 12] = struct.pack("<I", 1)
    protective_size = min(total_sectors - 1, 0xFFFFFFFF)
    mbr[0x1BE + 12:0x1BE + 16] = struct.pack("<I", protective_size)
    mbr[510:512] = b"\x55\xaa"
    f.seek(0)
    f.write(mbr)

    entry_count = 128
    entry_size = 128
    entries = bytearray(entry_count * entry_size)
    name = "NEXBOOT".encode("utf-16le")
    entry = bytearray(entry_size)
    entry[0:16] = esp_type
    entry[16:32] = part_guid
    entry[32:40] = struct.pack("<Q", part_start_lba)
    entry[40:48] = struct.pack("<Q", part_end_lba)
    entry[56:56 + len(name)] = name
    entries[0:entry_size] = entry
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    def make_header(current_lba, backup_lba, entries_lba):
        header = bytearray(sector_size)
        header[0:8] = b"EFI PART"
        header[8:12] = struct.pack("<I", 0x00010000)
        header[12:16] = struct.pack("<I", 92)
        header[24:32] = struct.pack("<Q", current_lba)
        header[32:40] = struct.pack("<Q", backup_lba)
        header[40:48] = struct.pack("<Q", 34)
        header[48:56] = struct.pack("<Q", part_end_lba)
        header[56:72] = disk_guid
        header[72:80] = struct.pack("<Q", entries_lba)
        header[80:84] = struct.pack("<I", entry_count)
        header[84:88] = struct.pack("<I", entry_size)
        header[88:92] = struct.pack("<I", entries_crc)
        crc = zlib.crc32(header[:92]) & 0xFFFFFFFF
        header[16:20] = struct.pack("<I", crc)
        return header

    primary_entries_lba = 2
    backup_entries_lba = last_lba - 32
    f.seek(primary_entries_lba * sector_size)
    f.write(entries)
    f.seek(backup_entries_lba * sector_size)
    f.write(entries)
    f.seek(sector_size)
    f.write(make_header(1, last_lba, primary_entries_lba))
    f.seek(last_lba * sector_size)
    f.write(make_header(last_lba, 1, backup_entries_lba))
PY

MTOOLS_IMAGE="${DISK_IMG}@@${PART_OFFSET_BYTES}"
mformat -i "$MTOOLS_IMAGE" -F -T "$PART_SECTORS" -v NEXBOOT ::
mmd -i "$MTOOLS_IMAGE" ::/EFI >/dev/null 2>&1 || true
mmd -i "$MTOOLS_IMAGE" ::/EFI/BOOT >/dev/null 2>&1 || true
mmd -i "$MTOOLS_IMAGE" ::/ISO >/dev/null 2>&1 || true
mcopy -o -i "$MTOOLS_IMAGE" "$EFI_FILE" ::/EFI/BOOT/BOOTX64.EFI

for image in "${IMAGES[@]}"; do
    name="$(basename "$image")"
    warn "Copying test image to /ISO/${name}..."
    mcopy -o -i "$MTOOLS_IMAGE" "$image" "::/ISO/${name}"
done

info "Disk image created: ${DISK_IMG}"

QEMU_OPTS=(
    -machine q35,accel=tcg
    -m "$MEMORY"
    -net none
    -nographic
    -serial mon:stdio
)

OVMF_PATHS=(
    "/usr/share/OVMF/OVMF_CODE.fd"
    "/usr/share/ovmf/OVMF.fd"
    "/usr/share/qemu/OVMF.fd"
    "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"
    "/opt/homebrew/opt/qemu/share/qemu/edk2-x86_64-code.fd"
)

OVMF_CODE=""
for path in "${OVMF_PATHS[@]}"; do
    if [ -f "$path" ]; then
        OVMF_CODE="$path"
        break
    fi
done

if [ -n "$OVMF_CODE" ]; then
    QEMU_OPTS+=(-drive "if=pflash,format=raw,readonly=on,file=${OVMF_CODE}")
elif [ "$NO_RUN" -eq 0 ]; then
    die "OVMF firmware not found. Install OVMF/edk2-ovmf or use --no-run to only create the image."
fi

case "$BUS" in
    virtio)
        QEMU_OPTS+=(
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "virtio-blk-pci,drive=nextboot_disk,bootindex=1"
        )
        ;;
    nvme)
        QEMU_OPTS+=(
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1"
        )
        ;;
    sata)
        QEMU_OPTS+=(
            -device "ahci,id=ahci0"
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "ide-hd,drive=nextboot_disk,bus=ahci0.0,bootindex=1"
        )
        ;;
    usb)
        QEMU_OPTS+=(
            -device "qemu-xhci,id=xhci"
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "usb-storage,drive=nextboot_disk,bootindex=1"
        )
        ;;
esac

echo -e "${BLUE}QEMU command:${NC}"
printf 'qemu-system-x86_64'
for opt in "${QEMU_OPTS[@]}"; do
    printf ' %q' "$opt"
done
printf '\n'

if [ "$NO_RUN" -eq 1 ]; then
    warn "--no-run set; image is ready for manual testing."
    exit 0
fi

require_command qemu-system-x86_64 "qemu-system-x86_64 is required to run the VM"

if [ -n "$OVMF_CODE" ]; then
    info "Using OVMF: ${OVMF_CODE}"
fi
warn "Starting QEMU. Press Ctrl+A then X to exit."
qemu-system-x86_64 "${QEMU_OPTS[@]}"

echo ""
info "QEMU exited"
