#!/bin/bash
# NextBoot QEMU Test Script
#
# Requirements:
#   - QEMU with OVMF support
#   - Built NextBoot EFI binary
#
# Usage:
#   ./scripts/run-qemu.sh          - Run with default settings
#   ./scripts/run-qemu.sh release  - Run release build

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}NextBoot QEMU Test${NC}"
echo "=================="

# Determine build mode
BUILD_MODE="debug"
if [ "$1" = "release" ]; then
    BUILD_MODE="release"
fi

# Paths
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
EFI_FILE="${PROJECT_DIR}/target/x86_64-unknown-uefi/${BUILD_MODE}/nextboot-boot.efi"

# Check if EFI file exists
if [ ! -f "$EFI_FILE" ]; then
    echo -e "${RED}Error: EFI file not found: ${EFI_FILE}${NC}"
    echo "Please run ./scripts/build.sh first"
    exit 1
fi

# OVMF paths (common locations)
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

if [ -z "$OVMF_CODE" ]; then
    echo -e "${RED}Error: OVMF firmware not found${NC}"
    echo "Please install OVMF:"
    echo "  Ubuntu/Debian: sudo apt install ovmf"
    echo "  Fedora: sudo dnf install edk2-ovmf"
    echo "  macOS: brew install qemu"
    exit 1
fi

echo -e "${GREEN}Using OVMF: ${OVMF_CODE}${NC}"
echo -e "${GREEN}EFI file: ${EFI_FILE}${NC}"

# Create a temporary disk image with the EFI file
DISK_IMG="${PROJECT_DIR}/target/qemu_disk.img"
mkdir -p "${PROJECT_DIR}/target"

# Create a 64MB FAT32 disk image
echo -e "${YELLOW}Creating test disk image...${NC}"
dd if=/dev/zero of="$DISK_IMG" bs=1M count=64 2>/dev/null

# Format as FAT32 (macOS)
if [[ "$OSTYPE" == "darwin"* ]]; then
    # On macOS, use hdiutil
    DISK_IMG_MOUNT="/tmp/nextboot_mount"
    rm -rf "$DISK_IMG_MOUNT"
    mkdir -p "$DISK_IMG_MOUNT"

    # Create FAT32 image
    hdiutil create -size 64m -fs MS-DOS -volname NEXBOOT "$DISK_IMG" -ov 2>/dev/null || true

    # Mount and copy files
    # This is complex on macOS, so we'll use mtools if available
    if command -v mcopy &> /dev/null; then
        # Create directory structure
        echo -e "${YELLOW}Copying EFI file to disk image...${NC}"
        mmd -i "$DISK_IMG" ::/EFI
        mmd -i "$DISK_IMG" ::/EFI/BOOT
        mcopy -i "$DISK_IMG" "$EFI_FILE" ::/EFI/BOOT/BOOTX64.EFI
    else
        echo -e "${RED}Error: mtools not installed${NC}"
        echo "Install with: brew install mtools"
        exit 1
    fi
else
    # Linux
    mkfs.vfat -F 32 "$DISK_IMG" 2>/dev/null

    # Mount and copy files
    MOUNT_DIR="/tmp/nextboot_mount"
    sudo mkdir -p "$MOUNT_DIR"
    sudo mount -o loop "$DISK_IMG" "$MOUNT_DIR"
    sudo mkdir -p "$MOUNT_DIR/EFI/BOOT"
    sudo cp "$EFI_FILE" "$MOUNT_DIR/EFI/BOOT/BOOTX64.EFI"
    sudo umount "$MOUNT_DIR"
fi

echo -e "${GREEN}Disk image created: ${DISK_IMG}${NC}"

# Run QEMU
echo -e "${YELLOW}Starting QEMU...${NC}"
echo -e "${YELLOW}Press Ctrl+A then X to exit QEMU${NC}"
echo ""

# QEMU command
QEMU_OPTS=(
    -machine q35,accel=tcg
    -m 512M
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE"
    -drive file="$DISK_IMG",format=raw,if=virtio
    -net none
    -nographic
    -serial mon:stdio
)

# Run QEMU
qemu-system-x86_64 "${QEMU_OPTS[@]}"

echo ""
echo -e "${GREEN}QEMU exited${NC}"
