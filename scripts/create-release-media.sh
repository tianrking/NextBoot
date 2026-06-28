#!/usr/bin/env bash
# Create a customer-burnable NextBoot raw media image.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="${TARGET:-x86_64-unknown-uefi}"
MODE="release"
SIZE_MB="7000"
SECTOR_SIZE="512"
DATA_FS="exfat"
GROWABLE_MAX_SIZE_MB="16777216"
OUTPUT=""
EFI_OVERRIDE=""
VENTOY_ASSETS_DIR=""
SKIP_BUILD=0
EXTRA_EFI_OVERRIDE_COUNT=0
declare -a IMAGES=()
declare -a EXTRA_EFI_OVERRIDES=()
declare -a RELEASE_TARGETS=()
declare -a EFI_BOOT_NAMES=()
declare -a EFI_FILES=()

usage() {
    cat <<USAGE
NextBoot Release Media

Usage:
  $0 [options]

Options:
  --target TARGET       x86_64-unknown-uefi, i686-unknown-uefi, aarch64-unknown-uefi, or all
  --mode MODE           debug or release build artifact to embed (default: release)
  --size MB             raw disk image size in MiB (default: 7000, fits 8GB media)
  --sector-size BYTES   logical sector size: 512 or 4096 (default: 512)
  --data-fs FS          data partition filesystem: exfat or fat32 (default: exfat)
  --growable-max-size MB maximum target media size for growable exFAT (default: 16777216)
  --image PATH          preseed an image into /ISO; repeatable
  --output PATH         output .img path
  --efi PATH            use an explicit EFI binary instead of target/TARGET/MODE
  --extra-efi NAME=PATH add an extra fallback EFI loader for QA media; repeatable
  --ventoy-assets DIR   copy optional Ventoy runtime assets into /ventoy
  --skip-build          do not run scripts/build.sh before creating the image
  -h, --help            Show this help

The generated image contains a small FAT ESP and a user-visible Data partition with
/ISO already present. exFAT release media reserves growth metadata so NextBoot
can expand NEXTDATA after the image is written to larger storage. Users flash
the .img with a normal raw-image writer, then drag ISO, WIM, VHD, VHDX, IMG,
or EFI files into /ISO and boot from the device in UEFI.
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

configure_targets() {
    case "$TARGET" in
        x86_64-unknown-uefi)
            RELEASE_TARGETS=("x86_64-unknown-uefi")
            EFI_BOOT_NAMES=("BOOTX64.EFI")
            ;;
        i686-unknown-uefi)
            RELEASE_TARGETS=("i686-unknown-uefi")
            EFI_BOOT_NAMES=("BOOTIA32.EFI")
            ;;
        aarch64-unknown-uefi)
            RELEASE_TARGETS=("aarch64-unknown-uefi")
            EFI_BOOT_NAMES=("BOOTAA64.EFI")
            ;;
        all)
            RELEASE_TARGETS=("x86_64-unknown-uefi" "i686-unknown-uefi" "aarch64-unknown-uefi")
            EFI_BOOT_NAMES=("BOOTX64.EFI" "BOOTIA32.EFI" "BOOTAA64.EFI")
            ;;
        *) die "unsupported target: $TARGET" ;;
    esac
}

append_extra_efi_override() {
    local entry="$1"
    local name="${entry%%=*}"
    local source="${entry#*=}"
    [ "$name" != "$entry" ] || die "--extra-efi must be NAME=PATH"
    [ -n "$source" ] || die "--extra-efi source path is empty"
    name="$(printf '%s' "$name" | tr '[:lower:]' '[:upper:]')"
    case "$name" in
        BOOTX64.EFI|BOOTIA32.EFI|BOOTAA64.EFI) ;;
        *) die "--extra-efi name must be BOOTX64.EFI, BOOTIA32.EFI, or BOOTAA64.EFI" ;;
    esac
    [ -f "$source" ] || die "extra EFI file not found: $source"
    local existing
    for existing in "${EFI_BOOT_NAMES[@]}"; do
        [ "$existing" != "$name" ] || die "duplicate EFI boot loader: $name"
    done
    EFI_BOOT_NAMES+=("$name")
    EFI_FILES+=("$source")
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
        --growable-max-size)
            [ "$#" -ge 2 ] || die "--growable-max-size requires a value"
            GROWABLE_MAX_SIZE_MB="$2"
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
        --extra-efi)
            [ "$#" -ge 2 ] || die "--extra-efi requires NAME=PATH"
            EXTRA_EFI_OVERRIDES+=("$2")
            EXTRA_EFI_OVERRIDE_COUNT=$((EXTRA_EFI_OVERRIDE_COUNT + 1))
            shift 2
            ;;
        --ventoy-assets)
            [ "$#" -ge 2 ] || die "--ventoy-assets requires a directory"
            VENTOY_ASSETS_DIR="$2"
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
case "$GROWABLE_MAX_SIZE_MB" in
    ''|*[!0-9]*) die "--growable-max-size must be an integer MiB value" ;;
esac

configure_targets
if [ "$SKIP_BUILD" -eq 0 ] && [ -z "$EFI_OVERRIDE" ]; then
    TARGET="$TARGET" "$SCRIPT_DIR/build.sh" "$MODE"
fi

if [ -n "$EFI_OVERRIDE" ] && [ "$TARGET" = "all" ]; then
    die "--efi is only supported with a single --target; use --extra-efi for QA multi-loader media"
fi

EFI_FILES=()
if [ -n "$EFI_OVERRIDE" ]; then
    EFI_FILES=("$EFI_OVERRIDE")
else
    for release_target in "${RELEASE_TARGETS[@]}"; do
        EFI_FILES+=("${PROJECT_DIR}/target/${release_target}/${MODE}/nextboot-boot.efi")
    done
fi

for efi_file in "${EFI_FILES[@]}"; do
    [ -f "$efi_file" ] || die "EFI file not found: $efi_file"
done
if [ "$EXTRA_EFI_OVERRIDE_COUNT" -gt 0 ]; then
    for extra_efi in "${EXTRA_EFI_OVERRIDES[@]}"; do
        append_extra_efi_override "$extra_efi"
    done
fi

EFI_FILE="${EFI_FILES[0]}"
BOOT_NAME="${EFI_BOOT_NAMES[0]}"
EXTRA_EFI_SPEC=""
if [ "${#EFI_FILES[@]}" -gt 1 ]; then
    for index in $(seq 1 $((${#EFI_FILES[@]} - 1))); do
        entry="${EFI_BOOT_NAMES[$index]}=${EFI_FILES[$index]}"
        if [ -z "$EXTRA_EFI_SPEC" ]; then
            EXTRA_EFI_SPEC="$entry"
        else
            EXTRA_EFI_SPEC="${EXTRA_EFI_SPEC};${entry}"
        fi
    done
fi

IMAGE_COUNT="${#IMAGES[@]}"
if [ "$IMAGE_COUNT" -gt 0 ]; then
    for image in "${IMAGES[@]}"; do
        [ -f "$image" ] || die "image file not found: $image"
    done
fi

if [ -z "$OUTPUT" ]; then
    if [ "$TARGET" = "all" ]; then
        OUTPUT_NAME="nextboot-universal-uefi"
    else
        OUTPUT_NAME="nextboot-${TARGET}"
    fi
    OUTPUT="${PROJECT_DIR}/target/release-media/${OUTPUT_NAME}.img"
fi
mkdir -p "$(dirname "$OUTPUT")"

CREATE_ARGS=(
    "$OUTPUT" "$SIZE_MB" "$SECTOR_SIZE" split "$DATA_FS" "$EFI_FILE" \
    0 0 0 "" "" 0 0 "$BOOT_NAME" "$EXTRA_EFI_SPEC"
)
if [ "$IMAGE_COUNT" -gt 0 ]; then
    CREATE_ARGS+=("${IMAGES[@]}")
fi
if [ "$DATA_FS" = "exfat" ]; then
    NEXTBOOT_GROWABLE_EXFAT=1 \
    NEXTBOOT_GROWABLE_EXFAT_MAX_MIB="$GROWABLE_MAX_SIZE_MB" \
    NEXTBOOT_VENTOY_ASSETS_DIR="$VENTOY_ASSETS_DIR" \
        python3 "$SCRIPT_DIR/qemu/create-disk-image.py" "${CREATE_ARGS[@]}"
else
    NEXTBOOT_VENTOY_ASSETS_DIR="$VENTOY_ASSETS_DIR" \
        python3 "$SCRIPT_DIR/qemu/create-disk-image.py" "${CREATE_ARGS[@]}"
fi

VERIFY_ARGS=(
    --disk-image "$OUTPUT"
    --sector-size "$SECTOR_SIZE"
    --layout split
    --data-fs "$DATA_FS"
    --efi-file "$EFI_FILE"
    --efi-boot-name "$BOOT_NAME"
)
if [ "${#EFI_FILES[@]}" -gt 1 ]; then
    for index in $(seq 1 $((${#EFI_FILES[@]} - 1))); do
        VERIFY_ARGS+=(--efi-loader "${EFI_BOOT_NAMES[$index]}=${EFI_FILES[$index]}")
    done
fi
if [ "$IMAGE_COUNT" -gt 0 ]; then
    for image in "${IMAGES[@]}"; do
        VERIFY_ARGS+=(--image "$image")
    done
fi
python3 "$SCRIPT_DIR/verify-qemu-image.py" "${VERIFY_ARGS[@]}"

echo "Wrote release media image: $OUTPUT"
echo "Embedded ${#EFI_FILES[@]} UEFI fallback loader(s): ${EFI_BOOT_NAMES[*]}"
if [ "$DATA_FS" = "exfat" ]; then
    echo "Growable NEXTDATA target ceiling: ${GROWABLE_MAX_SIZE_MB} MiB."
fi
echo "Flash this .img with a normal raw-image writer, then drag boot images into /ISO."
