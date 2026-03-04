#!/bin/bash
# 写入 U 盘脚本
# 用法: ./scripts/flash.sh /dev/sdX

set -e

DEVICE="$1"

if [ -z "$DEVICE" ]; then
    echo "用法: $0 /dev/sdX"
    echo ""
    echo "可用设备:"
    lsblk -d -o NAME,SIZE,MODEL,TRAN | grep -E "disk|NAME"
    exit 1
fi

# 安全检查
if [[ "$DEVICE" =~ ^/dev/(sd[a-z]|nvme[0-9]n[0-9]|mmcblk[0-9])$ ]]; then
    echo "⚠️  警告: 这将清除 $DEVICE 上的所有数据!"
    read -p "确认继续? (yes/no): " confirm
    if [ "$confirm" != "yes" ]; then
        echo "已取消"
        exit 0
    fi
else
    echo "❌ 无效的设备路径: $DEVICE"
    exit 1
fi

# 检查设备是否存在
if [ ! -b "$DEVICE" ]; then
    echo "❌ 设备不存在: $DEVICE"
    exit 1
fi

# 检查是否已挂载
if mount | grep -q "$DEVICE"; then
    echo "❌ 设备已挂载，请先卸载:"
    mount | grep "$DEVICE"
    exit 1
fi

echo "📦 准备分区表..."

# 创建 GPT 分区表
parted -s "$DEVICE" mklabel gpt

# 创建 ESP 分区 (200MB)
parted -s "$DEVICE" mkpart ESP fat32 1MiB 201MiB
parted -s "$DEVICE" set 1 esp on

# 创建 Data 分区 (剩余空间)
parted -s "$DEVICE" mkpart Data ext4 201MiB 100%

# 等待设备节点出现
sleep 2

# 确定分区设备名
if [[ "$DEVICE" =~ nvme|mmcblk ]]; then
    PART1="${DEVICE}p1"
    PART2="${DEVICE}p2"
else
    PART1="${DEVICE}1"
    PART2="${DEVICE}2"
fi

echo "📦 格式化分区..."

# 格式化 ESP
mkfs.vfat -F 32 -n "NEXTBOOT-EFI" "$PART1"

# 格式化 Data 分区
mkfs.exfat -n "NEXTBOOT-DATA" "$PART2" || {
    echo "exFAT 不支持，使用 NTFS..."
    mkfs.ntfs -f -L "NEXTBOOT-DATA" "$PART2"
}

echo "📦 安装 Bootloader..."

# 挂载 ESP
MOUNT_DIR=$(mktemp -d)
mount "$PART1" "$MOUNT_DIR"

# 创建目录结构
mkdir -p "$MOUNT_DIR/EFI/BOOT"

# 复制 bootloader
if [ -f "output/BOOTX64.EFI" ]; then
    cp "output/BOOTX64.EFI" "$MOUNT_DIR/EFI/BOOT/"
else
    echo "❌ Bootloader 不存在，请先运行 ./scripts/build.sh"
    umount "$MOUNT_DIR"
    rmdir "$MOUNT_DIR"
    exit 1
fi

# 创建 ISO 目录结构 (在 Data 分区)
umount "$MOUNT_DIR"
mount "$PART2" "$MOUNT_DIR"
mkdir -p "$MOUNT_DIR/ISO"
umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"

echo "✅ 安装完成!"
echo ""
echo "下一步:"
echo "1. 将 ISO 文件复制到 Data 分区的 /ISO 目录"
echo "2. 从此 U 盘启动"
echo ""
echo "分区信息:"
parted -s "$DEVICE" print
