#!/bin/bash
# NextBoot 构建脚本
# 用法: ./scripts/build.sh [release|debug]

set -e

MODE="${1:-release}"
TARGET="x86_64-unknown-uefi"
OUT_DIR="target/${TARGET}/${MODE}"

echo "🔧 Building NextBoot (${MODE})..."

# 检查 rust-src 组件
if ! rustup component list | grep -q "rust-src.*installed"; then
    echo "📦 Installing rust-src component..."
    rustup component add rust-src
fi

# 构建
if [ "$MODE" = "release" ]; then
    cargo build --target "$TARGET" --release
else
    cargo build --target "$TARGET"
fi

# 复制输出
EFI_FILE="${OUT_DIR}/nextboot-boot.efi"
if [ -f "$EFI_FILE" ]; then
    mkdir -p output
    cp "$EFI_FILE" "output/BOOTX64.EFI"
    echo "✅ Build complete: output/BOOTX64.EFI"
    ls -lh "output/BOOTX64.EFI"
else
    echo "❌ Build failed: EFI file not found"
    exit 1
fi
