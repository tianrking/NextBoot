#!/usr/bin/env bash
# NextBoot QEMU Test Script
#
# Creates GPT test disk images with NextBoot installed as the removable/fallback
# UEFI bootloader.  Split layouts use a FAT32 ESP plus an exFAT, FAT32, or NTFS
# Data partition so fixed-disk, NVMe, SATA, USB, SD, and virtio paths can be tested
# without rewriting real media.
#
# Usage:
#   ./scripts/run-qemu.sh
#   ./scripts/run-qemu.sh release
#   ./scripts/run-qemu.sh --bus nvme --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus nvme --sector-size 4096 --no-run
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vlnk-iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-static-vdi
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
#   TARGET=i686-unknown-uefi ./scripts/run-qemu.sh --bus virtio --smoke-efi-iso
#   TARGET=aarch64-unknown-uefi ./scripts/run-qemu.sh --bus virtio --smoke-efi-iso

set -eo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="${TARGET:-x86_64-unknown-uefi}"
BUILD_MODE="debug"
BUS="virtio"
DISK_SIZE_MB=256
DISK_SIZE_SET=0
SECTOR_SIZE=512
LAYOUT="single"
DATA_FS="exfat"
DISK_IMG=""
NO_RUN=0
VERIFY_IMAGE=1
SMOKE=0
SMOKE_BOOT=0
SMOKE_EFI_ISO=0
SMOKE_VLNK_ISO=0
SMOKE_RAW_IMG=0
SMOKE_FIXED_VHD=0
SMOKE_DYNAMIC_VHD=0
SMOKE_VHDX=0
SMOKE_SPARSE_VHDX=0
SMOKE_PARTIAL_VHDX=0
SMOKE_PARENT_VHDX=0
SMOKE_VDI=0
SMOKE_STATIC_VDI=0
SMOKE_SPARSE_VDI=0
SMOKE_DISCARDED_VDI=0
SMOKE_PARENT_VDI=0
SMOKE_AUTO_MEMDISK=0
SMOKE_MENU_MEMDISK=0
SMOKE_WINDOWS_ISO=0
SMOKE_WINDOWS_WIMBOOT=0
SMOKE_LINUX_ISO=0
SMOKE_LINUX_PLUGINS=0
SMOKE_HELPER_FILE=""
SMOKE_TIMEOUT=20
MEMORY="1024M"
IMAGES=()

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

source "${SCRIPT_DIR}/qemu/usage.sh"
source "${SCRIPT_DIR}/qemu/options.sh"
source "${SCRIPT_DIR}/qemu/arch.sh"
source "${SCRIPT_DIR}/qemu/device.sh"
source "${SCRIPT_DIR}/qemu/validate.sh"
source "${SCRIPT_DIR}/qemu/smoke-images.sh"
source "${SCRIPT_DIR}/qemu/run-smoke.sh"

parse_qemu_args "$@"

validate_qemu_args
configure_qemu_arch

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    die "EFI file not found: ${EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
fi

create_generated_smoke_images
SMOKE_VLNK_FILE=""
if [ "$SMOKE_VLNK_ISO" -eq 1 ]; then
    SMOKE_VLNK_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}-vlnk.vlnk.iso"
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
info "UEFI target: ${TARGET}"
info "Fallback loader: EFI/BOOT/${EFI_BOOT_NAME}"
info "Storage bus: ${BUS}"
info "Sector size: ${SECTOR_SIZE}"
info "Disk layout: ${LAYOUT}"
if [ "$LAYOUT" = "split" ]; then
    info "Data filesystem: ${DATA_FS}"
fi
info "Disk image: ${DISK_IMG}"

require_command python3 "python3 is required to create the GPT disk image"

warn "Creating ${LAYOUT} GPT test disk image..."
PY_ARGS=(
    "$DISK_IMG"
    "$DISK_SIZE_MB"
    "$SECTOR_SIZE"
    "$LAYOUT"
    "$DATA_FS"
    "$EFI_FILE"
    "$SMOKE_LINUX_PLUGINS"
    "$SMOKE_WINDOWS_WIMBOOT"
    "$SMOKE_VLNK_ISO"
    "$SMOKE_VLNK_FILE"
    "$SMOKE_HELPER_FILE"
    "$SMOKE_AUTO_MEMDISK"
    "$EFI_BOOT_NAME"
)
if [ "${#IMAGES[@]}" -gt 0 ]; then
    PY_ARGS+=("${IMAGES[@]}")
fi
CREATE_DISK_SCRIPT="${SCRIPT_DIR}/qemu/create-disk-image.py"
[ -f "$CREATE_DISK_SCRIPT" ] || die "QEMU disk creator not found: ${CREATE_DISK_SCRIPT}"
python3 "$CREATE_DISK_SCRIPT" "${PY_ARGS[@]}"

info "Disk image created: ${DISK_IMG}"

if [ "$VERIFY_IMAGE" -eq 1 ]; then
    VERIFY_SCRIPT="${SCRIPT_DIR}/verify-qemu-image.py"
    [ -f "$VERIFY_SCRIPT" ] || die "QEMU image verifier not found: ${VERIFY_SCRIPT}"
    warn "Verifying GPT/filesystem layout..."
    VERIFY_ARGS=(
        --disk-image "$DISK_IMG"
        --sector-size "$SECTOR_SIZE"
        --layout "$LAYOUT"
        --data-fs "$DATA_FS"
        --efi-file "$EFI_FILE"
        --efi-boot-name "$EFI_BOOT_NAME"
    )
    if [ "$SMOKE_VLNK_ISO" -eq 1 ]; then
        VERIFY_ARGS+=(--image "$SMOKE_VLNK_FILE")
    else
        for image in "${IMAGES[@]}"; do
            VERIFY_ARGS+=(--image "$image")
        done
    fi
    python3 "$VERIFY_SCRIPT" "${VERIFY_ARGS[@]}"
fi

QEMU_OPTS+=(
    -m "$MEMORY"
    -net none
    -nographic
    -serial mon:stdio
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

append_qemu_storage_device "$BUS" "$DISK_IMG" "$SECTOR_SIZE"

echo -e "${BLUE}QEMU command:${NC}"
printf '%s' "$QEMU_BINARY"
for opt in "${QEMU_OPTS[@]}"; do
    printf ' %q' "$opt"
done
printf '\n'

if [ "$NO_RUN" -eq 1 ]; then
    warn "--no-run set; image is ready for manual testing."
    exit 0
fi

require_command "$QEMU_BINARY" "${QEMU_BINARY} is required to run the VM"

if [ -n "$OVMF_CODE" ]; then
    info "Using OVMF: ${OVMF_CODE}"
fi

if [ "$SMOKE" -eq 1 ]; then
    run_qemu_smoke
    exit 0
fi

warn "Starting QEMU. Press Ctrl+A then X to exit."
"$QEMU_BINARY" "${QEMU_OPTS[@]}"

echo ""
info "QEMU exited"
