# NextBoot

**无需格式化 U 盘的 UEFI 启动加载器**

## 项目简介

NextBoot 是一个基于 Rust 的 UEFI 启动加载器，核心功能是无需格式化 U 盘，通过拖入 ISO 文件并模拟虚拟光驱来启动操作系统。

## 功能特性

- ✅ 支持 FAT32/exFAT/ext4/NTFS/UDF/ISO9660 文件系统
- ✅ GPT 分区表解析
- ✅ 虚拟 Block IO 设备模拟
- ✅ 图形化菜单界面 (GOP)
- ✅ Linux 发行版引导支持
- ✅ Windows ISO 引导支持
- ✅ 4K Native 设备兼容
- ✅ 固定盘/移动盘路径覆盖：NVMe、SATA、USB、SD、virtio

## 项目结构

```
NextBoot/
├── crates/
│   ├── nextboot-boot/     # 主 bootloader 入口
│   ├── nextboot-fs/       # 文件系统模块 (FAT32, exFAT, ext4, NTFS, UDF, ISO9660)
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

# 用 4K Native NVMe 固定盘路径测试
./scripts/run-qemu.sh --bus nvme --sector-size 4096 --no-run

# 用真实 SSD 风格的 ESP + exFAT Data 双分区 GPT 布局测试
./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --image ~/Downloads/ubuntu.iso

# 用 SD 控制器风格路径测试
./scripts/run-qemu.sh --bus sd --layout split --data-fs fat32 --smoke-efi-iso

# 用 NTFS Data 分区覆盖 Windows/大文件盘常见布局
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-linux-plugins

# 用 UDF Data 分区覆盖 Ventoy 风格的额外数据盘格式
./scripts/run-qemu.sh --bus nvme --layout split --data-fs udf --sector-size 4096 --smoke-efi-iso

# 用 ext4 Data 分区覆盖 Linux/SSD 常见数据盘格式
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ext4 --sector-size 4096 --smoke-efi-iso

# 启动 QEMU 并自动断言 NextBoot 扫描到镜像、进入菜单
./scripts/run-qemu.sh --bus nvme --layout split --sector-size 4096 --image ~/Downloads/ubuntu.iso --smoke

# 进一步自动按 Enter，断言选中镜像后安装虚拟 Block IO
./scripts/run-qemu.sh --bus nvme --layout split --sector-size 4096 --image ~/Downloads/ubuntu.iso --smoke-boot

# 自动生成一个带 EFI El Torito 的最小 ISO，并断言 ISO 内 BOOTX64.EFI 被链式启动
./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso

# 自动生成 Windows 风格 ISO，并断言 DVD-ROM/bootmgfw.efi 分支被链式启动
./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-windows-iso

# 自动生成 Ventoy .vlnk 指针文件，真实 ISO 隐藏在 /ventoy 下，并断言 vlnk 目标被启动
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-vlnk-iso

# 自动生成无默认 Windows EFI loader 的 ISO，并断言 WIMBOOT fallback 分支被链式启动
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot

# 自动生成 Linux 风格 ISO，并断言 EFI stub/initrd LoadFile2 分支被启动
./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso

# 自动生成 Linux ISO 和 Ventoy 插件载荷，并断言 persistence/injection/DUD/autoinstall 进入 initrd
./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins

# 只生成 GPT/FAT32 测试盘，不启动虚拟机
./scripts/run-qemu.sh --bus sata --no-run

# 运行推荐 QEMU smoke 矩阵；设置 NEXTBOOT_FULL_QEMU_MATRIX=1 可扩展到更多总线
./scripts/qemu-smoke-matrix.sh
```

`run-qemu.sh` 会直接创建 GPT/FAT32/exFAT/ext4/NTFS/UDF 磁盘镜像，并支持 `virtio`、`nvme`、`sata`、
`usb`、`sd` 五种 QEMU 存储路径，用来覆盖固定盘、可移动盘和 SD 控制器路径的启动差异。`--sector-size
4096` 会生成 4K Native 测试盘，并让 QEMU 设备暴露 4096B logical/physical block
size，用来复现新 SSD 和高性能移动硬盘常见的扇区尺寸差异。`--layout split` 会
生成独立 ESP 和 Data 分区：ESP 只放 `BOOTX64.EFI`，Data 分区放 `/ISO`，用于验证
固定盘上“引导分区与镜像分区分离”的真实部署路径；`--data-fs exfat|ext4|fat32|ntfs|udf` 可覆盖
默认写盘布局、Linux ext4 数据盘、FAT32 兼容布局、NTFS 大文件布局和 UDF 数据盘布局。生成后脚本会调用
`verify-qemu-image.py` 校验 GPT CRC、分区布局、FAT32/exFAT/ext4/NTFS/UDF 目录、`BOOTX64.EFI`、
`/ISO` 文件和物理 extent；`--smoke`
会继续启动 QEMU 并检查 NextBoot 日志里是否进入扫描/菜单阶段；`--smoke-boot` 会
自动按 Enter 并检查是否安装虚拟 Block IO；`--smoke-efi-iso` 会生成一个带 EFI
El Torito boot catalog、内含 `/EFI/BOOT/BOOTX64.EFI` 的最小 ISO，并继续验证该 EFI
loader 被链式启动；`--smoke-vlnk-iso` 会生成 Ventoy 兼容 `.vlnk.iso` 指针文件，
指向同一 Data 分区 `/ventoy/vlnk-target.iso` 下的真实 ISO，并验证 VLNK 解析、
目标 extent 映射、VentoyOsParam 的 vlnk 标记和目标 ISO 的 EFI loader 启动；
`--smoke-windows-iso` 会生成 Windows 风格 ISO，验证 Windows/WinPE
的 DVD-ROM 虚拟设备与 `/efi/microsoft/boot/bootmgfw.efi` 链式启动路径；
`--smoke-windows-wimboot` 会生成一个没有默认 Windows EFI loader、但包含 `boot.wim`、BCD
和 `boot.sdi` 的 ISO，同时在 Data 分区放入 `/ventoy/wimboot.x86_64.xz`，验证 Windows ISO
WIMBOOT fallback 入口；
`--smoke-linux-iso` 会生成不含 `/EFI/BOOT/BOOTX64.EFI` 的 Linux 风格 ISO，验证内核
候选发现、initrd LoadFile2 provider 和 EFI stub 启动路径；`--smoke-linux-plugins`
会额外生成 `/ventoy/ventoy.json`、自动安装模板、注入包、DUD 镜像和 persistence 后端，
验证 Ventoy Linux initrd overlay 能加载这些插件载荷；必要时可用 `--skip-verify` 跳过
镜像结构检查。

`qemu-smoke-matrix.sh` 默认执行最关键的固定盘与可移动盘组合：NVMe 4K split/exFAT
真实启动、USB 512 split/FAT32 真实启动，以及 SD 512 split/FAT32 带小 ISO 的镜像
生成与校验。`NEXTBOOT_FULL_QEMU_MATRIX=1 ./scripts/qemu-smoke-matrix.sh` 会继续覆盖
virtio、SATA/NTFS、NVMe/UDF 和 NVMe/ext4。SD 目前限制为 512B 扇区，因为 QEMU `sd-card` 设备没有提供和
NVMe/virtio/USB 相同的 logical block size override；SATA 也限制为 512B，因为 QEMU
`ide-hd` 要求 512B discard granularity。当前 macOS Homebrew OVMF 也不会直接从
`sdhci-pci` 启动，所以 SD 启动 smoke 需要显式设置 `NEXTBOOT_QEMU_SD_BOOT_SMOKE=1`
作为实验项。

### 写入 U 盘

```bash
# 列出可用设备
./scripts/flash.sh list

# 写入标准 ESP + Data 双分区布局
./scripts/flash.sh --layout split --data-fs exfat /dev/sdX

# 写入 NTFS Data 分区布局，适合 Windows/大文件盘工作流
./scripts/flash.sh --layout split --data-fs ntfs /dev/sdX

# 写入 ext4 Data 分区布局，适合 Linux SSD/NVMe 工作流
./scripts/flash.sh --layout split --data-fs ext4 /dev/nvme0n1

# 写入 UDF Data 分区布局，适合 Ventoy 风格兼容性验证
./scripts/flash.sh --layout split --data-fs udf /dev/sdX

# 显式指定 Ventoy 资产目录，安装 Windows WIMBOOT fallback 所需文件
./scripts/flash.sh --layout split --ventoy-assets ../Ventoy/INSTALL/ventoy /dev/sdX

# 先预览将要执行的分区/格式化命令
./scripts/flash.sh --dry-run --layout split /dev/sdX
```

`flash.sh` 默认使用 split GPT 布局：第 1 分区是 FAT32 ESP，只保存 NextBoot
启动文件；第 2 分区是 Data 分区，默认 exFAT，也可选择 FAT32、ext4、NTFS 或 UDF，用来存放
`/ISO` 下的 ISO/WIM/VHD 文件。旧式单分区 FAT32 仍可通过 `--layout single` 生成。
在 Linux 上写入 ext4/UDF Data 分区需要 `mkfs.ext4`/`mkudffs`。在 macOS 上写入 NTFS
Data 分区需要额外安装 `mkfs.ntfs`/`mkntfs`；若要脚本自动创建 `/ISO` 目录，还需要可写
NTFS 驱动，例如 `ntfs-3g`。macOS 没有可靠内置 ext4 写挂载，因此 `--data-fs ext4`
会完成分区和格式化，但需要从 Linux 往 Data 分区复制 ISO。脚本会自动探测
`../Ventoy/INSTALL/ventoy` 并把 `wimboot.x86_64.xz`、`vtoyjump64.exe`、`common_bcd.xz` 安装到
镜像所在卷的 `/ventoy` 目录；也可以用 `--ventoy-assets DIR` 指定目录，或用
`--no-ventoy-assets` 跳过。

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
