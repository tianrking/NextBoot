#!/usr/bin/env bash
# Create a customer-burnable NextBoot raw media image.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="${TARGET:-x86_64-unknown-uefi}"
MODE="release"
SIZE_MB="1024"
SECTOR_SIZE="512"
DATA_FS="exfat"
OUTPUT=""
EFI_OVERRIDE=""
SKIP_BUILD=0
IMAGES=()

usage() {
    cat <<USAGE
NextBoot Release Media

Usage:
  $0 [options]

Options:
  --target TARGET       x86_64-unknown-uefi, i686-unknown-uefi, or aarch64-unknown-uefi
  --mode MODE           debug or release build artifact to embed (default: release)
  --size MB             raw disk image size in MiB (default: 1024)
  --sector-size BYTES   logical sector size: 512 or 4096 (default: 512)
  --data-fs FS          data partition filesystem: exfat or fat32 (default: exfat)
  --image PATH          preseed an image into /ISO; repeatable
  --output PATH         output .img path
  --efi PATH            use an explicit EFI binary instead of target/TARGET/MODE
  --skip-build          do not run scripts/build.sh before creating the image
  -h, --help            Show this help

The generated image contains a FAT32 ESP and a user-visible Data partition with
/ISO already present. Users burn the .img to USB/SSD/SD media, then drag ISO,
WIM, VHD, VHDX, IMG, or EFI files into /ISO and boot from the device in UEFI.
USAGE
}

die() {
    echo "error: $*" >&2
    exit 1
}

boot_name_for_target() {
    case "$1" in
        x86_64-unknown-uefi) printf 'BOOTX64.EFI\n' ;;
        i686-unknown-uefi) printf 'BOOTIA32.EFI\n' ;;
        aarch64-unknown-uefi) printf 'BOOTAA64.EFI\n' ;;
        *) die "unsupported target: $1" ;;
    esac
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            [ "$#" -ge 2 ] || die "--target requires a value"
            TARGET="$2"
            shift 2
            ;;
        --mode)
            [ "$#" -ge 2 ] || die "--mode requires a value"
            MODE="$2"
            shift 2
            ;;
        --size)
            [ "$#" -ge 2 ] || die "--size requires a value"
            SIZE_MB="$2"
            shift 2
            ;;
        --sector-size)
            [ "$#" -ge 2 ] || die "--sector-size requires a value"
            SECTOR_SIZE="$2"
            shift 2
            ;;
        --data-fs)
            [ "$#" -ge 2 ] || die "--data-fs requires a value"
            DATA_FS="$2"
            shift 2
            ;;
        --image)
            [ "$#" -ge 2 ] || die "--image requires a file path"
            IMAGES+=("$2")
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || die "--output requires a path"
            OUTPUT="$2"
            shift 2
            ;;
        --efi)
            [ "$#" -ge 2 ] || die "--efi requires a file path"
            EFI_OVERRIDE="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

case "$MODE" in
    debug|release) ;;
    *) die "--mode must be debug or release" ;;
esac
case "$SIZE_MB" in
    ''|*[!0-9]*) die "--size must be an integer MiB value" ;;
esac
case "$SECTOR_SIZE" in
    512|4096) ;;
    *) die "--sector-size must be 512 or 4096" ;;
esac
case "$DATA_FS" in
    exfat|fat32) ;;
    *) die "--data-fs must be exfat or fat32 for customer release media" ;;
esac

BOOT_NAME="$(boot_name_for_target "$TARGET")"
if [ "$SKIP_BUILD" -eq 0 ] && [ -z "$EFI_OVERRIDE" ]; then
    TARGET="$TARGET" "$SCRIPT_DIR/build.sh" "$MODE"
fi

EFI_FILE="${EFI_OVERRIDE:-${PROJECT_DIR}/target/${TARGET}/${MODE}/nextboot-boot.efi}"
[ -f "$EFI_FILE" ] || die "EFI file not found: $EFI_FILE"
IMAGE_COUNT="${#IMAGES[@]}"
if [ "$IMAGE_COUNT" -gt 0 ]; then
    for image in "${IMAGES[@]}"; do
        [ -f "$image" ] || die "image file not found: $image"
    done
fi

if [ -z "$OUTPUT" ]; then
    OUTPUT="${PROJECT_DIR}/target/release-media/nextboot-${TARGET}-${SECTOR_SIZE}b-${DATA_FS}.img"
fi
mkdir -p "$(dirname "$OUTPUT")"

CREATE_ARGS=(
    "$OUTPUT" "$SIZE_MB" "$SECTOR_SIZE" split "$DATA_FS" "$EFI_FILE" \
    0 0 0 "" "" 0 "$BOOT_NAME"
)
if [ "$IMAGE_COUNT" -gt 0 ]; then
    CREATE_ARGS+=("${IMAGES[@]}")
fi
python3 "$SCRIPT_DIR/qemu/create-disk-image.py" "${CREATE_ARGS[@]}"

VERIFY_ARGS=(
    --disk-image "$OUTPUT"
    --sector-size "$SECTOR_SIZE"
    --layout split
    --data-fs "$DATA_FS"
    --efi-file "$EFI_FILE"
    --efi-boot-name "$BOOT_NAME"
)
if [ "$IMAGE_COUNT" -gt 0 ]; then
    for image in "${IMAGES[@]}"; do
        VERIFY_ARGS+=(--image "$image")
    done
fi
python3 "$SCRIPT_DIR/verify-qemu-image.py" "${VERIFY_ARGS[@]}"

echo "Wrote release media image: $OUTPUT"
echo "Burn this .img to USB/SSD/SD media, then drag boot images into /ISO."
