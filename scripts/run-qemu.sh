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
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
#   ./scripts/run-qemu.sh --bus usb --mode release --no-run

set -eo pipefail

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
SMOKE_VDI=0
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
source "${SCRIPT_DIR}/qemu/device.sh"
source "${SCRIPT_DIR}/qemu/smoke-images.sh"
source "${SCRIPT_DIR}/qemu/run-smoke.sh"

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
            DISK_SIZE_SET=1
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
        --data-fs)
            [ $# -ge 2 ] || die "--data-fs requires a value"
            DATA_FS="$2"
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
        --skip-verify)
            VERIFY_IMAGE=0
            shift
            ;;
        --smoke)
            SMOKE=1
            shift
            ;;
        --smoke-boot)
            SMOKE=1
            SMOKE_BOOT=1
            shift
            ;;
        --smoke-efi-iso)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            shift
            ;;
        --smoke-vlnk-iso)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_VLNK_ISO=1
            shift
            ;;
        --smoke-raw-img)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_RAW_IMG=1
            shift
            ;;
        --smoke-vhd)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_FIXED_VHD=1
            shift
            ;;
        --smoke-dynamic-vhd)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_DYNAMIC_VHD=1
            shift
            ;;
        --smoke-vdi)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_VDI=1
            shift
            ;;
        --smoke-auto-memdisk)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_AUTO_MEMDISK=1
            shift
            ;;
        --smoke-menu-memdisk)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_MENU_MEMDISK=1
            shift
            ;;
        --smoke-windows-iso)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_WINDOWS_ISO=1
            shift
            ;;
        --smoke-windows-wimboot)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_WINDOWS_ISO=1
            SMOKE_WINDOWS_WIMBOOT=1
            shift
            ;;
        --smoke-linux-iso)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_LINUX_ISO=1
            shift
            ;;
        --smoke-linux-plugins)
            SMOKE=1
            SMOKE_BOOT=1
            SMOKE_EFI_ISO=1
            SMOKE_LINUX_ISO=1
            SMOKE_LINUX_PLUGINS=1
            shift
            ;;
        --smoke-timeout)
            [ $# -ge 2 ] || die "--smoke-timeout requires a value"
            SMOKE_TIMEOUT="$2"
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

validate_qemu_storage_bus "$BUS"

case "$DISK_SIZE_MB" in
    ''|*[!0-9]*) die "--disk-size must be an integer MiB value" ;;
esac

case "$SECTOR_SIZE" in
    512|4096) ;;
    *) die "--sector-size must be 512 or 4096" ;;
esac
validate_qemu_bus_sector_size "$BUS" "$SECTOR_SIZE"

case "$SMOKE_TIMEOUT" in
    ''|*[!0-9]*) die "--smoke-timeout must be an integer second value" ;;
esac

case "$LAYOUT" in
    single|split) ;;
    *) die "--layout must be single or split" ;;
esac

case "$DATA_FS" in
    exfat|ext2|ext3|ext4|fat32|ntfs|udf|xfs) ;;
    *) die "--data-fs must be exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs" ;;
esac

if { [[ "$DATA_FS" == ext* ]] || [ "$DATA_FS" = "xfs" ]; } && [ "$SECTOR_SIZE" -ne 4096 ]; then
    die "--data-fs ext2/ext3/ext4/xfs currently requires --sector-size 4096 in the QEMU generator"
fi

if [ "$LAYOUT" = "single" ] && [ "$DATA_FS" != "exfat" ]; then
    warn "--data-fs is ignored for single layout"
fi

if [ "$SMOKE" -eq 1 ] && [ "$NO_RUN" -eq 1 ] && [ "$SMOKE_EFI_ISO" -eq 0 ] && [ "$SMOKE_RAW_IMG" -eq 0 ] && [ "$SMOKE_FIXED_VHD" -eq 0 ] && [ "$SMOKE_DYNAMIC_VHD" -eq 0 ] && [ "$SMOKE_VDI" -eq 0 ]; then
    die "--smoke without a generated smoke image cannot be combined with --no-run"
fi

if [ "$SMOKE_WINDOWS_ISO" -eq 1 ] && [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
    die "--smoke-windows-iso and --smoke-linux-iso cannot be combined"
fi

if { [ "$SMOKE_RAW_IMG" -eq 1 ] || [ "$SMOKE_FIXED_VHD" -eq 1 ] || [ "$SMOKE_DYNAMIC_VHD" -eq 1 ] || [ "$SMOKE_VDI" -eq 1 ]; } && [ "$SMOKE_EFI_ISO" -eq 1 ]; then
    die "--smoke-raw-img/--smoke-vhd/--smoke-dynamic-vhd/--smoke-vdi cannot be combined with ISO smoke generators"
fi

SMOKE_DISK_IMAGE_COUNT=$((SMOKE_RAW_IMG + SMOKE_FIXED_VHD + SMOKE_DYNAMIC_VHD + SMOKE_VDI))
if [ "$SMOKE_DISK_IMAGE_COUNT" -gt 1 ]; then
    die "--smoke-raw-img, --smoke-vhd, --smoke-dynamic-vhd, and --smoke-vdi are mutually exclusive"
fi

if [ "$SMOKE_BOOT" -eq 1 ] && [ "$SMOKE_EFI_ISO" -eq 0 ] && [ "$SMOKE_RAW_IMG" -eq 0 ] && [ "$SMOKE_FIXED_VHD" -eq 0 ] && [ "$SMOKE_DYNAMIC_VHD" -eq 0 ] && [ "$SMOKE_VDI" -eq 0 ] && [ "${#IMAGES[@]}" -eq 0 ]; then
    die "--smoke-boot requires at least one --image"
fi

if [ "$DISK_SIZE_SET" -eq 0 ]; then
    if [ "$SECTOR_SIZE" -eq 4096 ]; then
        if [ "$LAYOUT" = "split" ]; then
            DISK_SIZE_MB=1024
        else
            DISK_SIZE_MB=512
        fi
    fi
fi

MIN_DISK_SIZE_MB=64
if [ "$LAYOUT" = "split" ]; then
    MIN_DISK_SIZE_MB=128
fi
if [ "$SECTOR_SIZE" -eq 4096 ]; then
    MIN_DISK_SIZE_MB=260
    if [ "$LAYOUT" = "split" ]; then
        MIN_DISK_SIZE_MB=544
    fi
fi
if [ "$DISK_SIZE_MB" -lt "$MIN_DISK_SIZE_MB" ]; then
    die "--disk-size must be at least ${MIN_DISK_SIZE_MB} MiB for ${LAYOUT} layout with ${SECTOR_SIZE}B sectors"
fi

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    die "EFI file not found: ${EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
fi

create_generated_smoke_images
SMOKE_VLNK_FILE=""
if [ "$SMOKE_VLNK_ISO" -eq 1 ]; then
    SMOKE_VLNK_FILE="${PROJECT_DIR}/target/nextboot-smoke-vlnk.vlnk.iso"
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

append_qemu_storage_device "$BUS" "$DISK_IMG" "$SECTOR_SIZE"

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

if [ "$SMOKE" -eq 1 ]; then
    run_qemu_smoke
    exit 0
fi

warn "Starting QEMU. Press Ctrl+A then X to exit."
qemu-system-x86_64 "${QEMU_OPTS[@]}"

echo ""
info "QEMU exited"
