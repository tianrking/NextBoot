#!/bin/bash
# QEMU 测试脚本
# 用法: ./scripts/run-qemu.sh [disk.img]

set -e

DISK_IMAGE="${1:-test/disk.img}"
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE.fd}"
OVMF_VARS="${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS.fd}"

# 检查 OVMF
if [ ! -f "$OVMF_CODE" ]; then
    echo "❌ OVMF not found. Install with:"
    echo "   Ubuntu/Debian: sudo apt install ovmf"
    echo "   Fedora: sudo dnf install edk2-ovmf"
    echo "   macOS: brew install qemu (includes OVMF)"
    exit 1
fi

# 创建测试磁盘镜像 (如果不存在)
if [ ! -f "$DISK_IMAGE" ]; then
    echo "📦 Creating test disk image..."
    mkdir -p "$(dirname "$DISK_IMAGE")"
    qemu-img create -f raw "$DISK_IMAGE" 2G
fi

# 创建 ESP 分区结构
ESP_DIR="test/esp"
mkdir -p "$ESP_DIR/EFI/BOOT"

# 复制 bootloader
if [ -f "output/BOOTX64.EFI" ]; then
    cp "output/BOOTX64.EFI" "$ESP_DIR/EFI/BOOT/"
else
    echo "⚠️  No bootloader found. Run ./scripts/build.sh first"
fi

# 创建 ISO 目录结构
mkdir -p "$ESP_DIR/ISO"

echo "🚀 Starting QEMU..."
echo "   Disk: $DISK_IMAGE"
echo "   OVMF: $OVMF_CODE"

# 运行 QEMU
qemu-system-x86_64 \
    -machine q35,accel=tcg \
    -cpu qemu64 \
    -m 2G \
    -bios "$OVMF_CODE" \
    -drive file="$DISK_IMAGE",format=raw,if=none,id=boot \
    -device ahci,id=ahci \
    -device ide-hd,drive=boot,bus=ahci.0 \
    -serial stdio \
    -no-reboot \
    -no-shutdown \
    -net none \
    "$@"

echo "QEMU exited"
