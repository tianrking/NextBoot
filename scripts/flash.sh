#!/bin/bash
# NextBoot Flash Script
#
# Write NextBoot to a USB drive
#
# Usage:
#   ./scripts/flash.sh <device>    - Flash to specified device
#   ./scripts/flash.sh list        - List available devices
#
# Example:
#   ./scripts/flash.sh /dev/sdX

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo -e "${GREEN}NextBoot Flash Tool${NC}"
echo "===================="

# Function to list devices
list_devices() {
    echo -e "${BLUE}Available storage devices:${NC}"
    echo ""

    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        diskutil list external | grep -E "^/dev/disk" || echo "No external drives found"
    else
        # Linux
        lsblk -o NAME,SIZE,TYPE,MOUNTPOINT -d | grep -E "disk|sd|usb" || echo "No drives found"
    fi

    echo ""
    echo -e "${YELLOW}Usage: $0 /dev/sdX${NC}"
}

# Check if no arguments
if [ $# -eq 0 ]; then
    list_devices
    exit 0
fi

# List devices if requested
if [ "$1" = "list" ]; then
    list_devices
    exit 0
fi

DEVICE="$1"

# Verify device exists
if [ ! -e "$DEVICE" ]; then
    echo -e "${RED}Error: Device not found: ${DEVICE}${NC}"
    exit 1
fi

# EFI file
EFI_FILE="${PROJECT_DIR}/target/x86_64-unknown-uefi/release/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    EFI_FILE="${PROJECT_DIR}/target/x86_64-unknown-uefi/debug/nextboot-boot.efi"
fi

if [ ! -f "$EFI_FILE" ]; then
    echo -e "${RED}Error: EFI file not found${NC}"
    echo "Please run ./scripts/build.sh first"
    exit 1
fi

echo -e "${YELLOW}EFI file: ${EFI_FILE}${NC}"
echo -e "${YELLOW}Target device: ${DEVICE}${NC}"
echo ""

# Confirm
echo -e "${RED}WARNING: This will ERASE ALL DATA on ${DEVICE}${NC}"
echo -n "Are you sure? (yes/no): "
read -r CONFIRM

if [ "$CONFIRM" != "yes" ]; then
    echo "Aborted"
    exit 0
fi

# Unmount device if mounted
echo -e "${YELLOW}Unmounting device...${NC}"
if [[ "$OSTYPE" == "darwin"* ]]; then
    diskutil unmountDisk "$DEVICE" 2>/dev/null || true
else
    sudo umount "$DEVICE"* 2>/dev/null || true
fi

# Create partition table
echo -e "${YELLOW}Creating GPT partition table...${NC}"
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS
    sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT 100%
else
    # Linux
    sudo parted -s "$DEVICE" mklabel gpt
    sudo parted -s "$DEVICE" mkpart primary fat32 1MiB 100%
    sudo parted -s "$DEVICE" set 1 esp on
    sudo mkfs.vfat -F 32 "${DEVICE}1"
fi

# Mount and copy files
echo -e "${YELLOW}Copying files...${NC}"
MOUNT_DIR="/tmp/nextboot_flash"

if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS - the partition is already mounted by diskutil
    PARTITION="${DEVICE}s1"
    if [ ! -e "$PARTITION" ]; then
        PARTITION="${DEVICE}"
    fi

    # Find mount point
    MOUNT_POINT=$(df | grep "$PARTITION" | awk '{print $NF}')

    if [ -z "$MOUNT_POINT" ]; then
        echo -e "${RED}Error: Could not find mount point${NC}"
        exit 1
    fi

    echo -e "${GREEN}Mounted at: ${MOUNT_POINT}${NC}"

    # Create directory structure
    mkdir -p "${MOUNT_POINT}/EFI/BOOT"
    cp "$EFI_FILE" "${MOUNT_POINT}/EFI/BOOT/BOOTX64.EFI"

    # Create ISO directory
    mkdir -p "${MOUNT_POINT}/ISO"

    # Sync and unmount
    sync
    diskutil unmount "$PARTITION"
else
    # Linux
    sudo mkdir -p "$MOUNT_DIR"
    sudo mount "${DEVICE}1" "$MOUNT_DIR"

    # Create directory structure
    sudo mkdir -p "${MOUNT_DIR}/EFI/BOOT"
    sudo cp "$EFI_FILE" "${MOUNT_DIR}/EFI/BOOT/BOOTX64.EFI"

    # Create ISO directory
    sudo mkdir -p "${MOUNT_DIR}/ISO"

    # Sync and unmount
    sync
    sudo umount "$MOUNT_DIR"
fi

echo ""
echo -e "${GREEN}Flash complete!${NC}"
echo ""
echo "Your USB drive is now ready."
echo "Copy your ISO files to the /ISO directory and boot from USB."
