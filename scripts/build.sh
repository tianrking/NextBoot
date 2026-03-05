#!/bin/bash
# NextBoot Build Script
#
# Usage:
#   ./scripts/build.sh          - Build in debug mode
#   ./scripts/build.sh release  - Build in release mode

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}NextBoot Build Script${NC}"
echo "========================"

# Check if Rust is installed
if ! command -v rustc &> /dev/null; then
    echo -e "${RED}Error: Rust is not installed${NC}"
    echo "Please install Rust from https://rustup.rs/"
    exit 1
fi

# Check if the UEFI target is installed
if ! rustup target list | grep -q "x86_64-unknown-uefi (installed)"; then
    echo -e "${YELLOW}Installing x86_64-unknown-uefi target...${NC}"
    rustup target add x86_64-unknown-uefi
fi

# Check if rust-src is installed
if ! rustup component list | grep -q "rust-src (installed)"; then
    echo -e "${YELLOW}Installing rust-src component...${NC}"
    rustup component add rust-src
fi

# Determine build mode
BUILD_MODE="debug"
if [ "$1" = "release" ]; then
    BUILD_MODE="release"
fi

echo -e "${YELLOW}Building in ${BUILD_MODE} mode...${NC}"

# Build the project
if [ "$BUILD_MODE" = "release" ]; then
    cargo build --target x86_64-unknown-uefi --release
else
    cargo build --target x86_64-unknown-uefi
fi

# Check if build succeeded
if [ $? -eq 0 ]; then
    echo -e "${GREEN}Build successful!${NC}"

    # Output file location
    OUTPUT_DIR="target/x86_64-unknown-uefi/${BUILD_MODE}"
    OUTPUT_FILE="${OUTPUT_DIR}/nextboot-boot.efi"

    if [ -f "$OUTPUT_FILE" ]; then
        SIZE=$(du -h "$OUTPUT_FILE" | cut -f1)
        echo -e "${GREEN}Output: ${OUTPUT_FILE} (${SIZE})${NC}"
    fi
else
    echo -e "${RED}Build failed!${NC}"
    exit 1
fi
