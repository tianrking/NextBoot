#!/usr/bin/env bash
# NextBoot QEMU Test Script
#
# Creates GPT test disk images with NextBoot installed as the removable/fallback
# UEFI bootloader.  Split layouts use a small FAT ESP plus an exFAT, FAT32, or NTFS
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
SMOKE_PARENT_PARTIAL_VHDX=0
SMOKE_PARENT_CHAIN_VHDX=0
SMOKE_MISSING_PARENT_VHDX=0
SMOKE_VDI=0
SMOKE_STATIC_VDI=0
SMOKE_SPARSE_VDI=0
SMOKE_DISCARDED_VDI=0
SMOKE_PARENT_VDI=0
SMOKE_PARENT_CHAIN_VDI=0
SMOKE_MISSING_PARENT_VDI=0
SMOKE_PARENT_CHAIN_DEPTH=2
SMOKE_AUTO_MEMDISK=0
SMOKE_MENU_MEMDISK=0
SMOKE_CONF_REPLACE=0
SMOKE_WINDOWS_ISO=0
SMOKE_WINDOWS_WIMBOOT=0
SMOKE_LINUX_ISO=0
SMOKE_LINUX_PLUGINS=0
SMOKE_LINUX_GRUB=0
SMOKE_HELPER_FILE=""
SMOKE_ARTIFACT_TAG="${NEXTBOOT_SMOKE_ARTIFACT_TAG:-}"
SMOKE_ARTIFACT_SUFFIX=""
SMOKE_TIMEOUT=20
MEMORY="1024M"
IMAGES=()
SUPPORT_IMAGES=()

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

# Honour PYTHON so Windows users can point Git Bash at a real interpreter.
# The Microsoft Store `python3` app-execution alias exists on PATH on many
# systems but is only a redirector and exits unsuccessfully outside its setup
# flow.  Checking a trivial Python program prevents a misleading, silent
# partial QEMU setup in that case.
PYTHON_BIN="${PYTHON:-python3}"

require_python() {
    require_command "$PYTHON_BIN" "Python 3 is required. Install Python 3 or set PYTHON to its executable path."
    "$PYTHON_BIN" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1 || \
        die "${PYTHON_BIN} cannot run Python 3. Set PYTHON to a working Python 3 executable (the Windows Store python3 alias is not sufficient)."
}

source "${SCRIPT_DIR}/qemu/usage.sh"
source "${SCRIPT_DIR}/qemu/options.sh"
source "${SCRIPT_DIR}/qemu/arch.sh"
source "${SCRIPT_DIR}/qemu/device.sh"
source "${SCRIPT_DIR}/qemu/validate.sh"
source "${SCRIPT_DIR}/qemu/smoke-images.sh"
source "${SCRIPT_DIR}/qemu/run-smoke.sh"

require_python

parse_qemu_args "$@"

validate_qemu_args
configure_qemu_arch

# The official Windows QEMU installer does not always add its directory to the
# PATH inherited by Git Bash.  Prefer its normal installation path when the
# selected emulator is otherwise unavailable; callers can still supply QEMU on
# PATH for custom installations.
if ! command -v "$QEMU_BINARY" >/dev/null 2>&1; then
    WINDOWS_QEMU_BINARY="/c/Program Files/qemu/${QEMU_BINARY}.exe"
    if [ -x "$WINDOWS_QEMU_BINARY" ]; then
        QEMU_BINARY="$WINDOWS_QEMU_BINARY"
    fi
fi
if [ -n "$SMOKE_ARTIFACT_TAG" ]; then
    case "$SMOKE_ARTIFACT_TAG" in
        *[!A-Za-z0-9_.-]*)
            die "NEXTBOOT_SMOKE_ARTIFACT_TAG may only contain letters, digits, dot, underscore, or dash"
            ;;
    esac
    SMOKE_ARTIFACT_SUFFIX="-${SMOKE_ARTIFACT_TAG}"
fi

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    die "EFI file not found: ${EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
fi

create_generated_smoke_images
SMOKE_VLNK_FILE=""
if [ "$SMOKE_VLNK_ISO" -eq 1 ]; then
    SMOKE_VLNK_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-vlnk.vlnk.iso"
fi

for image in "${IMAGES[@]}" "${SUPPORT_IMAGES[@]}"; do
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

require_python

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
    "$SMOKE_CONF_REPLACE"
    "$EFI_BOOT_NAME"
    ""
)
if [ "${#IMAGES[@]}" -gt 0 ] || [ "${#SUPPORT_IMAGES[@]}" -gt 0 ]; then
    PY_ARGS+=("${IMAGES[@]}")
    PY_ARGS+=("${SUPPORT_IMAGES[@]}")
fi
CREATE_DISK_SCRIPT="${SCRIPT_DIR}/qemu/create-disk-image.py"
[ -f "$CREATE_DISK_SCRIPT" ] || die "QEMU disk creator not found: ${CREATE_DISK_SCRIPT}"
"$PYTHON_BIN" "$CREATE_DISK_SCRIPT" "${PY_ARGS[@]}"

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
        for image in "${IMAGES[@]}" "${SUPPORT_IMAGES[@]}"; do
            VERIFY_ARGS+=(--image "$image")
        done
    fi
    "$PYTHON_BIN" "$VERIFY_SCRIPT" "${VERIFY_ARGS[@]}"
fi

QEMU_OPTS+=(
    -m "$MEMORY"
    -net none
    -nographic
    -serial mon:stdio
)

# On Windows, QEMU does not reliably route stdin from a non-interactive
# process to the UEFI console.  QMP injects a real emulated key event after
# the menu marker is observed, so the smoke runner tests the selected boot
# path instead of stopping at the menu.
QMP_PORT=""
if [ "$SMOKE" -eq 1 ] && [ "$SMOKE_BOOT" -eq 1 ]; then
    QMP_PORT="${NEXTBOOT_QEMU_QMP_PORT:-4444}"
    case "$QMP_PORT" in
        ''|*[!0-9]*|0) die "NEXTBOOT_QEMU_QMP_PORT must be a TCP port number" ;;
    esac
    if [ "$QMP_PORT" -gt 65535 ]; then
        die "NEXTBOOT_QEMU_QMP_PORT must be between 1 and 65535"
    fi
    QEMU_OPTS+=( -qmp "tcp:127.0.0.1:${QMP_PORT},server=on,wait=off" )
fi

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
