# NextBoot

**无需格式化 U 盘的 UEFI 启动加载器**

## 项目简介

NextBoot 是一个基于 Rust 的 UEFI 启动加载器，核心功能是无需格式化 U 盘，通过拖入 ISO 文件并模拟虚拟光驱来启动操作系统。

## 功能特性

- ✅ 支持 FAT32/exFAT/ISO9660 文件系统
- ✅ GPT 分区表解析
- ✅ 虚拟 Block IO 设备模拟
- ✅ 图形化菜单界面 (GOP)
- ✅ Linux 发行版引导支持
- ✅ Windows ISO 引导支持
- ✅ 4K Native 设备兼容

## 项目结构

```
NextBoot/
├── crates/
│   ├── nextboot-boot/     # 主 bootloader 入口
│   ├── nextboot-fs/       # 文件系统模块 (FAT32, exFAT, ISO9660)
│   ├── nextboot-virtio/   # 虚拟 Block IO 驱动
│   ├── nextboot-menu/     # UEFI GOP 菜单渲染
│   ├── nextboot-linux/    # Linux 引导支持
│   └── nextboot-windows/  # Windows 引导支持
├── docs/
│   ├── ARCHITECTURE.md    # 架构设计文档
│   └── progress/          # 开发进度记录
├── scripts/
│   ├── build.sh           # 构建脚本
│   ├── run-qemu.sh        # QEMU 测试脚本
│   └── flash.sh           # 写入 U 盘脚本
└── Cargo.toml             # Workspace 配置
```

## 快速开始

### 环境要求

- Rust 1.70+ (安装 `rustup`)
- QEMU + OVMF (用于测试)
- x86_64-unknown-uefi target

### 安装依赖

```bash
# 添加 UEFI 目标
rustup target add x86_64-unknown-uefi

# 安装 rust-src 组件
rustup component add rust-src

# 安装 QEMU (macOS)
brew install qemu

# 安装 OVMF (Linux)
# Ubuntu/Debian:
sudo apt install ovmf
# Fedora:
sudo dnf install edk2-ovmf
```

### 构建

```bash
# 仅检查 UEFI 目标是否能通过类型检查
./scripts/build.sh check

# Debug 模式
./scripts/build.sh

# Release 模式
./scripts/build.sh release

# 可选：覆盖目标平台
TARGET=x86_64-unknown-uefi ./scripts/build.sh check
```

`scripts/build.sh` 会优先验证当前 toolchain 是否已经包含 UEFI target，并在需要时安装
`x86_64-unknown-uefi`。如果本机的 `rustup` shim 状态异常，也可以显式指定真实工具链：

```bash
RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc" \
CARGO="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo" \
./scripts/build.sh check
```

### 测试

```bash
# 使用 QEMU 测试
./scripts/run-qemu.sh

# 用 NVMe 固定盘路径测试，并把 ISO 复制进 /ISO
./scripts/run-qemu.sh --bus nvme --image ~/Downloads/ubuntu.iso

# 只生成 GPT/FAT32 测试盘，不启动虚拟机
./scripts/run-qemu.sh --bus sata --no-run
```

`run-qemu.sh` 会直接创建 GPT/FAT32 磁盘镜像，并支持 `virtio`、`nvme`、`sata`、
`usb` 四种 QEMU 存储路径，用来覆盖固定盘和可移动盘的启动差异。

### 写入 U 盘

```bash
# 列出可用设备
./scripts/flash.sh list

# 写入指定设备
./scripts/flash.sh /dev/sdX
```

## 使用方法

1. 将 NextBoot 写入 U 盘
2. 将 ISO 文件复制到 U 盘的 `/ISO` 目录
3. 从 U 盘启动
4. 在菜单中选择要启动的 ISO

## 支持的操作系统

### Linux
- Ubuntu
- Debian
- Fedora
- Arch Linux
- openSUSE
- CentOS
- 通用 Linux

### Windows
- Windows 10
- Windows 11
- WinPE

## 技术架构

详见 [ARCHITECTURE.md](docs/ARCHITECTURE.md)

## 开发进度

详见 [docs/progress/](docs/progress/)

- [MVP](docs/progress/MVP.md) - 基础功能
- [Beta](docs/progress/Beta.md) - Windows 支持优化

## 许可证

MIT OR Apache-2.0

## 贡献

欢迎提交 Issue 和 Pull Request！
