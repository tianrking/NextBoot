#!/usr/bin/env bash
# NextBoot Flash Script
#
# Creates a standard GPT disk for NextBoot.  The default split layout keeps the
# UEFI bootloader on a small FAT32 ESP and stores images on a separate data
# partition, which matches fixed-disk SSD/NVMe deployments better than a single
# all-purpose FAT32 volume.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
HOST_OS="${NEXTBOOT_OSTYPE:-$OSTYPE}"
TARGET="${TARGET:-x86_64-unknown-uefi}"

LAYOUT="split"
DATA_FS="exfat"
ESP_SIZE_MB=260
INSTALL_VENTOY_ASSETS=1
VENTOY_ASSETS_DIR=""
VENTOY_ASSETS_EXPLICIT=0
VENTOY_ASSETS_RESOLVED=""
DRY_RUN=0
ASSUME_YES=0

usage() {
    cat <<USAGE
NextBoot Flash Tool

Usage:
  $0 list
  $0 [options] <device>

Options:
  --target TARGET   UEFI build target: x86_64-unknown-uefi or aarch64-unknown-uefi
  --layout LAYOUT   Disk layout: split or single (default: split)
  --data-fs FS      Data partition filesystem for split layout: exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs (default: exfat)
  --esp-size MB     ESP size for split layout in MiB (default: 260)
  --ventoy-assets DIR
                    Install WIMBOOT assets from DIR into /ventoy
  --no-ventoy-assets
                    Do not auto-install Ventoy WIMBOOT assets
  --dry-run         Print the commands without writing to the device
  -y, --yes         Skip the confirmation prompt
  -h, --help        Show this help

Examples:
  $0 list
  $0 --layout split --data-fs exfat /dev/diskX
  $0 --layout split --data-fs ext3 /dev/nvme0n1
  $0 --layout split --data-fs ext4 /dev/nvme0n1
  $0 --layout split --data-fs ntfs /dev/diskX
  $0 --layout split --data-fs udf /dev/sdX
  $0 --layout split --data-fs xfs /dev/nvme0n1
  $0 --target aarch64-unknown-uefi --layout split --data-fs exfat /dev/diskX
  $0 --layout split --ventoy-assets ../Ventoy/INSTALL/ventoy /dev/diskX
  $0 --layout split --data-fs fat32 /dev/sdX
  $0 --layout single /dev/sdX
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

note() {
    echo -e "${BLUE}$*${NC}"
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

    echo ""
    warn "Usage: $0 --layout split /dev/diskX"
}

source "${SCRIPT_DIR}/lib/flash_helpers.sh"

configure_flash_target() {
    case "$TARGET" in
        x86_64-unknown-uefi)
            EFI_BOOT_NAME="BOOTX64.EFI"
            ;;
        aarch64-unknown-uefi)
            EFI_BOOT_NAME="BOOTAA64.EFI"
            ;;
        *)
            die "Unsupported UEFI target '${TARGET}'. Supported: x86_64-unknown-uefi, aarch64-unknown-uefi"
            ;;
    esac
}

parse_args() {
    if [ $# -eq 0 ]; then
        list_devices
        exit 0
    fi

    if [ "$1" = "list" ]; then
        list_devices
        exit 0
    fi

    DEVICE=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --target)
                [ $# -ge 2 ] || die "--target requires a value"
                TARGET="$2"
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
            --esp-size)
                [ $# -ge 2 ] || die "--esp-size requires a value"
                ESP_SIZE_MB="$2"
                shift 2
                ;;
            --ventoy-assets)
                [ $# -ge 2 ] || die "--ventoy-assets requires a directory"
                VENTOY_ASSETS_DIR="$2"
                VENTOY_ASSETS_EXPLICIT=1
                INSTALL_VENTOY_ASSETS=1
                shift 2
                ;;
            --no-ventoy-assets)
                INSTALL_VENTOY_ASSETS=0
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
                if [ -n "${DEVICE}" ]; then
                    die "Only one target device can be specified"
                fi
                DEVICE="$1"
                shift
                ;;
        esac
    done

    [ -n "$DEVICE" ] || die "Missing target device"
}

parse_args "$@"
configure_flash_target

case "$LAYOUT" in
    split|single) ;;
    *) die "--layout must be split or single" ;;
esac

case "$DATA_FS" in
    exfat|ext2|ext3|ext4|fat32|ntfs|udf|xfs) ;;
    *) die "--data-fs must be exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs" ;;
esac

case "$ESP_SIZE_MB" in
    ''|*[!0-9]*) die "--esp-size must be an integer MiB value" ;;
esac

if [ "$LAYOUT" = "single" ] && [ "$DATA_FS" != "exfat" ]; then
    warn "--data-fs is ignored for single layout"
fi

if [ "$ESP_SIZE_MB" -lt 64 ]; then
    die "--esp-size must be at least 64 MiB"
fi

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/release/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    EFI_FILE="${PROJECT_DIR}/target/${TARGET}/debug/nextboot-boot.efi"
fi

[ -f "$EFI_FILE" ] || die "EFI file not found. Run TARGET=${TARGET} ./scripts/build.sh first."

if [ "$INSTALL_VENTOY_ASSETS" -eq 1 ]; then
    if VENTOY_ASSETS_RESOLVED="$(resolve_ventoy_assets_dir)"; then
        :
    elif [ "$VENTOY_ASSETS_EXPLICIT" -eq 1 ]; then
        [ -d "$VENTOY_ASSETS_DIR" ] || die "Ventoy assets directory not found: ${VENTOY_ASSETS_DIR}"
        die "Missing ${VENTOY_ASSETS_DIR}/wimboot.x86_64.xz"
    else
        warn "Ventoy WIMBOOT assets were not found; Windows WIMBOOT fallback will need /ventoy/wimboot.x86_64.xz copied later."
    fi
fi

if [ "$DRY_RUN" -eq 0 ] && [ ! -e "$DEVICE" ]; then
    die "Device not found: ${DEVICE}"
fi

echo -e "${GREEN}NextBoot Flash Tool${NC}"
echo "===================="
warn "EFI file: ${EFI_FILE}"
warn "UEFI target: ${TARGET}"
warn "Fallback loader: EFI/BOOT/${EFI_BOOT_NAME}"
warn "Target device: ${DEVICE}"
warn "Layout: ${LAYOUT}"
if [ "$LAYOUT" = "split" ]; then
    warn "ESP size: ${ESP_SIZE_MB} MiB"
    warn "Data filesystem: ${DATA_FS}"
fi
if [ -n "$VENTOY_ASSETS_RESOLVED" ]; then
    warn "Ventoy assets: ${VENTOY_ASSETS_RESOLVED}"
elif [ "$INSTALL_VENTOY_ASSETS" -eq 0 ]; then
    warn "Ventoy assets: disabled"
fi
if [ "$DRY_RUN" -eq 1 ]; then
    note "Dry run: no commands will be executed"
fi
echo ""

if [ "$ASSUME_YES" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
    echo -e "${RED}WARNING: This will ERASE ALL DATA on ${DEVICE}${NC}"
    echo -n "Are you sure? (yes/no): "
    read -r CONFIRM
    [ "$CONFIRM" = "yes" ] || { echo "Aborted"; exit 0; }
fi

warn "Unmounting device..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    if [ "$DRY_RUN" -eq 0 ]; then
        require_macos_tools
    fi
    DEVICE="$(normalize_macos_device "$DEVICE")"
    run_cmd diskutil unmountDisk "$DEVICE" || true
else
    if [ "$DRY_RUN" -eq 0 ]; then
        require_linux_tools
    fi
    run_sudo umount "${DEVICE}"* || true
fi

warn "Creating GPT partition table..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    if [ "$LAYOUT" = "split" ]; then
        if [ "$DATA_FS" = "fat32" ]; then
            MAC_DATA_FS="FAT32"
        elif [ "$DATA_FS" = "ntfs" ]; then
            # diskutil on stock macOS cannot format NTFS.  Create a Microsoft
            # Basic Data placeholder, then reformat it with mkfs.ntfs/mkntfs.
            MAC_DATA_FS="ExFAT"
        elif [[ "$DATA_FS" == ext* ]] || [ "$DATA_FS" = "udf" ] || [ "$DATA_FS" = "xfs" ]; then
            # Create a mountable placeholder, then reformat it with the selected
            # external formatter so GPT geometry stays under diskutil control.
            MAC_DATA_FS="ExFAT"
        else
            MAC_DATA_FS="ExFAT"
        fi
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT "${ESP_SIZE_MB}MiB" "$MAC_DATA_FS" NEXTDATA R
        DATA_PART="${DEVICE}s2"
        if [ "$DATA_FS" = "ntfs" ]; then
            run_cmd diskutil unmount "$DATA_PART" || true
            NTFS_MKFS="$(ntfs_mkfs_command)"
            run_sudo "$NTFS_MKFS" -Q -F -L NEXTDATA "$DATA_PART"
        elif [[ "$DATA_FS" == ext* ]]; then
            run_cmd diskutil unmount "$DATA_PART" || true
            run_ext_mkfs "$DATA_FS" "$DATA_PART"
        elif [ "$DATA_FS" = "udf" ]; then
            run_cmd diskutil unmount "$DATA_PART" || true
            run_udf_mkfs "$DATA_PART"
        elif [ "$DATA_FS" = "xfs" ]; then
            run_cmd diskutil unmount "$DATA_PART" || true
            XFS_MKFS="$(xfs_mkfs_command)"
            run_sudo "$XFS_MKFS" -f -L NEXTDATA "$DATA_PART"
        fi
    else
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT 100%
    fi
else
    run_sudo parted -s "$DEVICE" mklabel gpt
    if [ "$LAYOUT" = "split" ]; then
        esp_end="${ESP_SIZE_MB}MiB"
        if [ "$DATA_FS" = "ntfs" ]; then
            parted_data_type="ntfs"
        elif [[ "$DATA_FS" == ext* ]]; then
            parted_data_type="$DATA_FS"
        elif [ "$DATA_FS" = "xfs" ]; then
            parted_data_type="xfs"
        else
            parted_data_type="fat32"
        fi
        run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB "$esp_end"
        run_sudo parted -s "$DEVICE" set 1 esp on
        run_sudo parted -s "$DEVICE" mkpart NEXTDATA "$parted_data_type" "$esp_end" 100%
    else
        run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB 100%
        run_sudo parted -s "$DEVICE" set 1 esp on
    fi
    run_sudo partprobe "$DEVICE" || true
    if [ "$DRY_RUN" -eq 0 ]; then
        sleep 2
    fi

    ESP_PART="$(linux_partition_path "$DEVICE" 1)"
    run_sudo mkfs.vfat -F 32 -n NEXBOOT "$ESP_PART"
    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="$(linux_partition_path "$DEVICE" 2)"
        if [ "$DATA_FS" = "exfat" ]; then
            if [ "$DRY_RUN" -eq 1 ]; then
                EXFAT_MKFS="mkfs.exfat"
            else
                EXFAT_MKFS="$(find_linux_exfat_mkfs)"
            fi
            run_sudo "$EXFAT_MKFS" -n NEXTDATA "$DATA_PART"
        elif [[ "$DATA_FS" == ext* ]]; then
            run_ext_mkfs "$DATA_FS" "$DATA_PART"
        elif [ "$DATA_FS" = "ntfs" ]; then
            NTFS_MKFS="$(ntfs_mkfs_command)"
            run_sudo "$NTFS_MKFS" -Q -F -L NEXTDATA "$DATA_PART"
        elif [ "$DATA_FS" = "udf" ]; then
            run_udf_mkfs "$DATA_PART"
        elif [ "$DATA_FS" = "xfs" ]; then
            XFS_MKFS="$(xfs_mkfs_command)"
            run_sudo "$XFS_MKFS" -f -L NEXTDATA "$DATA_PART"
        else
            run_sudo mkfs.vfat -F 32 -n NEXTDATA "$DATA_PART"
        fi
    fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
    echo ""
    info "Dry run complete. No data was written."
    exit 0
fi

warn "Copying files..."
if [[ "$HOST_OS" == "darwin"* ]]; then
    ESP_PART="${DEVICE}s1"
    ESP_MOUNT="$(ensure_macos_mounted "$ESP_PART")"
    copy_efi_tree "$ESP_MOUNT"

    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="${DEVICE}s2"
        if [[ "$DATA_FS" == ext* ]] || [ "$DATA_FS" = "xfs" ]; then
            warn "Skipping Data partition population on macOS ${DATA_FS}; copy ISO files into the Data partition from Linux."
        elif [ "$DATA_FS" = "ntfs" ] && command_exists ntfs-3g; then
            DATA_MOUNT="/tmp/nextboot_flash_data"
            run_sudo mkdir -p "$DATA_MOUNT"
            run_sudo ntfs-3g "$DATA_PART" "$DATA_MOUNT"
            run_sudo mkdir -p "${DATA_MOUNT}/ISO"
            copy_ventoy_assets_sudo "$DATA_MOUNT"
        else
            DATA_MOUNT="$(ensure_macos_mounted "$DATA_PART")"
            run_cmd mkdir -p "${DATA_MOUNT}/ISO"
            copy_ventoy_assets "$DATA_MOUNT"
        fi
        sync
        if [ "$DATA_FS" = "ntfs" ] && command_exists ntfs-3g; then
            run_sudo umount "$DATA_MOUNT"
        else
            run_cmd diskutil unmount "$DATA_PART"
        fi
    else
        run_cmd mkdir -p "${ESP_MOUNT}/ISO"
        copy_ventoy_assets "$ESP_MOUNT"
        sync
    fi
    run_cmd diskutil unmount "$ESP_PART"
else
    ESP_PART="$(linux_partition_path "$DEVICE" 1)"
    ESP_MOUNT="/tmp/nextboot_flash_esp"
    DATA_MOUNT="/tmp/nextboot_flash_data"

    run_sudo mkdir -p "$ESP_MOUNT"
    run_sudo mount "$ESP_PART" "$ESP_MOUNT"
    copy_efi_tree_sudo "$ESP_MOUNT"

    if [ "$LAYOUT" = "split" ]; then
        DATA_PART="$(linux_partition_path "$DEVICE" 2)"
        run_sudo mkdir -p "$DATA_MOUNT"
        run_sudo mount "$DATA_PART" "$DATA_MOUNT"
        run_sudo mkdir -p "${DATA_MOUNT}/ISO"
        copy_ventoy_assets_sudo "$DATA_MOUNT"
        sync
        run_sudo umount "$DATA_MOUNT"
    else
        run_sudo mkdir -p "${ESP_MOUNT}/ISO"
        copy_ventoy_assets_sudo "$ESP_MOUNT"
        sync
    fi
    run_sudo umount "$ESP_MOUNT"
fi

echo ""
info "Flash complete!"
echo ""
if [ "$LAYOUT" = "split" ]; then
    echo "Copy ISO/WIM/VHD files to the Data partition's /ISO directory and boot from the device."
else
    echo "Copy ISO/WIM/VHD files to /ISO and boot from the device."
fi
