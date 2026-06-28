# NextBoot

**无需格式化 U 盘的 UEFI 启动加载器**

## 项目简介

NextBoot 是一个基于 Rust 的 UEFI 启动加载器，核心功能是无需格式化 U 盘，通过拖入 ISO 文件并模拟虚拟光驱来启动操作系统。

## 功能特性

- ✅ 支持 FAT32/exFAT/ext2/ext3/ext4/NTFS/UDF/XFS/ISO9660 文件系统
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
│   ├── nextboot-fs/       # 文件系统模块 (FAT32, exFAT, ext2/3/4, NTFS, UDF, XFS, ISO9660)
│   ├── nextboot-virtio/   # 虚拟 Block IO 驱动
│   ├── nextboot-menu/     # UEFI GOP 菜单渲染
│   ├── nextboot-linux/    # Linux 引导支持
│   └── nextboot-windows/  # Windows 引导支持
├── docs/
│   ├── architecture.md    # 架构设计文档
│   ├── secure-boot.md     # Secure Boot 本地签名工作流
│   └── progress/          # 开发进度记录
├── scripts/
│   ├── build.sh           # 构建脚本
│   ├── secure-boot.sh     # Secure Boot 证书、签名和验签脚本
│   ├── run-qemu.sh        # QEMU 测试脚本
│   └── flash.sh           # 写入 U 盘脚本
└── Cargo.toml             # Workspace 配置
```

## 快速开始

### 环境要求

- Rust 1.70+ (安装 `rustup`)
- QEMU + OVMF (用于测试)
- x86_64-unknown-uefi target; 可选 `i686-unknown-uefi`、`aarch64-unknown-uefi`

### 安装依赖

```bash
# 添加 UEFI 目标
rustup target add x86_64-unknown-uefi
rustup target add i686-unknown-uefi
rustup target add aarch64-unknown-uefi

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

# 同时构建 x86_64、IA32 和 AArch64 UEFI 产物
TARGET=all ./scripts/build.sh

# Release 模式
./scripts/build.sh release

# 可选：覆盖目标平台
TARGET=x86_64-unknown-uefi ./scripts/build.sh check
TARGET=i686-unknown-uefi ./scripts/build.sh check
TARGET=aarch64-unknown-uefi ./scripts/build.sh check
TARGET=all ./scripts/build.sh check
```

`scripts/build.sh` 会优先验证当前 toolchain 是否已经包含 UEFI target，并在需要时安装
`x86_64-unknown-uefi`、`i686-unknown-uefi` 或 `aarch64-unknown-uefi`。如果本机的 `rustup` shim 状态异常，也可以显式指定真实工具链：

```bash
RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc" \
CARGO="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo" \
./scripts/build.sh check
```

### Secure Boot 本地签名

```bash
# 查看签名工具状态和默认路径
./scripts/secure-boot.sh status

# 生成本机测试证书和固件/MOK 可登记的 DER 证书
./scripts/secure-boot.sh generate-test-cert

# 构建并签名 EFI
./scripts/build.sh release
./scripts/secure-boot.sh sign

# 在安装了 sbverify 的环境验证签名
./scripts/secure-boot.sh verify
```

签名流程默认输出到 `target/secure-boot/`。把 `nextboot-db.cer` 登记到固件
Secure Boot `db` 或 shim MOK 后，再把 `nextboot-boot-signed.efi` 作为
`EFI/BOOT/BOOTX64.EFI` 安装到 ESP。这个流程适合自有设备和实验室环境；生产级
shim、微软 UEFI CA 签名和 SBAT/吊销策略仍在兼容性 gap 中。详细限制见
`docs/secure-boot.md`。

### 测试

```bash
# 轻量项目健康检查：500 行限制、Python 编译、shell 语法、host 单测、flash/QEMU 镜像/硬件门禁和 UEFI check
./scripts/check-project-health.py

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

# 用 IA32 UEFI 固件测试 virtio 固定盘路径和 BOOTIA32.EFI fallback
TARGET=i686-unknown-uefi ./scripts/build.sh
TARGET=i686-unknown-uefi ./scripts/run-qemu.sh --bus virtio --smoke-efi-iso

# 用 AArch64 UEFI 固件测试 virtio 固定盘路径和 BOOTAA64.EFI fallback
TARGET=aarch64-unknown-uefi ./scripts/build.sh
TARGET=aarch64-unknown-uefi ./scripts/run-qemu.sh --bus virtio --smoke-efi-iso

# 用 NTFS Data 分区覆盖 Windows/大文件盘常见布局
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-linux-plugins

# 用 UDF Data 分区覆盖 Ventoy 风格的额外数据盘格式
./scripts/run-qemu.sh --bus nvme --layout split --data-fs udf --sector-size 4096 --smoke-efi-iso

# 用 ext2 Data 分区覆盖老式 Linux 文件系统映射
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ext2 --sector-size 4096 --smoke-efi-iso

# 用 ext4 Data 分区覆盖 Linux/SSD 常见数据盘格式
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ext4 --sector-size 4096 --smoke-efi-iso

# 用 ext3 Data 分区覆盖老式 Linux 文件系统映射
./scripts/run-qemu.sh --bus nvme --layout split --data-fs ext3 --sector-size 4096 --smoke-efi-iso

# 用受限 XFS Data 分区覆盖 XFS extent 读取框架
./scripts/run-qemu.sh --bus nvme --layout split --data-fs xfs --sector-size 4096 --smoke-efi-iso

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

`run-qemu.sh` 会直接创建 GPT/FAT32/exFAT/ext2/ext3/ext4/NTFS/UDF/XFS 磁盘镜像，并支持 `virtio`、`nvme`、`sata`、
`usb`、`sd` 五种 QEMU 存储路径，用来覆盖固定盘、可移动盘和 SD 控制器路径的启动差异。`--sector-size
4096` 会生成 4K Native 测试盘，并让 QEMU 设备暴露 4096B logical/physical block
size，用来复现新 SSD 和高性能移动硬盘常见的扇区尺寸差异。`--layout split` 会
生成独立 ESP 和 Data 分区：ESP 只放当前目标架构的 fallback EFI（x86_64 为
`BOOTX64.EFI`，IA32 为 `BOOTIA32.EFI`，AArch64 为 `BOOTAA64.EFI`），Data 分区放 `/ISO`，用于验证
固定盘上“引导分区与镜像分区分离”的真实部署路径；`--data-fs exfat|ext2|ext3|ext4|fat32|ntfs|udf|xfs` 可覆盖
默认写盘布局、Linux ext 系列数据盘、FAT32 兼容布局、NTFS 大文件布局和 UDF 数据盘布局。生成后脚本会调用
`verify-qemu-image.py` 校验 GPT CRC、分区布局、FAT32/exFAT/ext2/3/4/NTFS/UDF/XFS 目录、fallback EFI、
`/ISO` 文件和物理 extent。XFS QEMU 路径覆盖 512B/4K 设备扇区上的 4K XFS 文件系统块、
真实 inode 编号映射、shortform 目录、dir2/dir3 block/data 目录和 NextBoot 小目录子集；真实
`mkfs.xfs` 更复杂的大目录/btree 形态仍在 gap 列表中；`--smoke`
会继续启动 QEMU 并检查 NextBoot 日志里是否进入扫描/菜单阶段；`--smoke-boot` 会
自动按 Enter 并检查是否安装虚拟 Block IO；`--smoke-efi-iso` 会生成一个带 EFI
El Torito boot catalog、内含当前架构 `/EFI/BOOT/BOOT*.EFI` fallback 的最小 ISO，并继续验证该 EFI
loader 被链式启动；`--smoke-vlnk-iso` 会生成 Ventoy 兼容 `.vlnk.iso` 指针文件，
指向同一 Data 分区 `/ventoy/vlnk-target.iso` 下的真实 ISO，并验证 VLNK 解析、
目标 extent 映射、VentoyOsParam 的 vlnk 标记和目标 ISO 的 EFI loader 启动；
`--smoke-raw-img` 会生成内层 GPT/FAT32 raw `.img`，验证 NextBoot 把它当作虚拟硬盘安装后从其 ESP 启动；
`--smoke-vhd` 会把同一个内层启动盘包成 fixed VHD，验证 VHD footer 识别、虚拟大小裁剪和虚拟硬盘启动；
`--smoke-dynamic-vhd` 会生成全分配 dynamic VHD，验证 BAT/bitmap 映射和虚拟硬盘启动；
`--smoke-vhdx` 会生成全分配 VHDX，验证 VHDX region table、metadata、BAT 映射和虚拟硬盘启动；
`--smoke-sparse-vhdx` 会把全零 payload block 编码成 VHDX `ZERO` BAT 状态，验证稀疏虚拟硬盘启动；
`--smoke-partial-vhdx` 会把 payload block 编码成 VHDX `PARTIALLY_PRESENT` 并写入全 present
sector bitmap，验证不依赖父盘数据的 partially-present VHDX 也能启动；
`--smoke-vdi` 会生成全分配 dynamic VDI，验证 VDI block map 映射和虚拟硬盘启动；
`--smoke-static-vdi` 会生成 static VDI，验证预分配 VDI 的 block map 与虚拟硬盘启动；
`--smoke-sparse-vdi` 会把全零 payload block 编码成 VDI unallocated block，验证稀疏虚拟硬盘启动；
`--smoke-discarded-vdi` 会把全零 payload block 编码成 VDI discarded block，验证 VirtualBox/QEMU 常见丢弃块语义；
`--smoke-windows-iso` 会生成 Windows 风格 ISO，验证 Windows/WinPE
的 DVD-ROM 虚拟设备与 `/efi/microsoft/boot/bootmgfw.efi` 链式启动路径；
`--smoke-windows-wimboot` 会生成一个没有默认 Windows EFI loader、但包含 `boot.wim`、BCD
和 `boot.sdi` 的 ISO，同时在 Data 分区放入 `/ventoy/wimboot.x86_64.xz`，验证 Windows ISO
WIMBOOT fallback 入口；
`--smoke-linux-iso` 会生成不含当前架构 fallback EFI 的 Linux 风格 ISO，验证内核
候选发现、initrd LoadFile2 provider 和 EFI stub 启动路径；`--smoke-linux-plugins`
会额外生成 `/ventoy/ventoy.json`、自动安装模板、注入包、DUD 镜像和 persistence 后端，
验证 Ventoy Linux initrd overlay 能加载这些插件载荷；必要时可用 `--skip-verify` 跳过
镜像结构检查。

`qemu-smoke-matrix.sh` 默认执行最关键的固定盘与可移动盘组合：NVMe 4K split/exFAT
真实启动、USB 512 split/FAT32 真实启动，以及 SD 512 split/FAT32 带小 ISO 的镜像
生成与校验。`NEXTBOOT_FULL_QEMU_MATRIX=1 ./scripts/qemu-smoke-matrix.sh` 会继续覆盖
virtio、SATA/NTFS、NVMe/UDF、NVMe/ext2/ext3、NVMe/XFS、NVMe 512B/XFS、NVMe/XFS VLNK、NVMe raw IMG、NVMe fixed/dynamic VHD、NVMe VHDX、NVMe sparse/partially-present VHDX、parent-required VHDX 负向路径、NVMe dynamic/static/sparse/discarded VDI、parent-required VDI 负向路径和 NVMe/ext4 Linux 插件载荷。SD 目前限制为 512B 扇区，因为 QEMU `sd-card` 设备没有提供和
NVMe/virtio/USB 相同的 logical block size override；SATA 也限制为 512B，因为 QEMU
`ide-hd` 要求 512B discard granularity。当前 macOS Homebrew OVMF 也不会直接从
`sdhci-pci` 启动，所以 SD 启动 smoke 需要显式设置 `NEXTBOOT_QEMU_SD_BOOT_SMOKE=1`
作为实验项。
矩阵默认使用 `x86_64-unknown-uefi`；可用 `TARGET=i686-unknown-uefi ./scripts/qemu-smoke-matrix.sh`
覆盖 IA32 virtio/NVMe/USB 等路径，或用 `TARGET=aarch64-unknown-uefi ./scripts/qemu-smoke-matrix.sh`
覆盖 AArch64 路径，其中 fallback 文件名会自动改为 `BOOTIA32.EFI` 或 `BOOTAA64.EFI`。

`./scripts/check-flash-dry-run.py` 会用 `flash.sh --dry-run --no-ventoy-assets`
检查真实写盘前的命令规划，覆盖 macOS `rdisk` 归一化、Linux NVMe `p1/p2`
分区后缀、普通 USB `/dev/sdX1` 后缀、SD/MMC `/dev/mmcblkXp1` 后缀、
split/single 布局和 `--target all` 多架构 ESP fallback 安装。

真实硬件测试用 `./scripts/hardware-report.sh` 生成统一报告，并可追加
`docs/hardware/hardware-matrix.csv`。推荐覆盖项见
`docs/hardware-compatibility-matrix.md`，包括内置 NVMe SSD、USB SSD 盒、传统 U 盘、
SATA SSD、SD 读卡器、512B/4K 扇区和 exFAT/NTFS/ext/UDF/XFS Data 分区组合。

### 写入 U 盘

```bash
# 列出可用设备
./scripts/flash.sh list

# 写入标准 ESP + Data 双分区布局
./scripts/flash.sh --layout split --data-fs exfat /dev/sdX

# 写入 AArch64 UEFI 介质，ESP 使用 EFI/BOOT/BOOTAA64.EFI
TARGET=aarch64-unknown-uefi ./scripts/flash.sh --layout split --data-fs exfat /dev/sdX

# 写入 IA32 UEFI 介质，ESP 使用 EFI/BOOT/BOOTIA32.EFI
TARGET=i686-unknown-uefi ./scripts/flash.sh --layout split --data-fs exfat /dev/sdX

# 写入跨架构介质，ESP 同时包含 BOOTX64.EFI、BOOTIA32.EFI 和 BOOTAA64.EFI
TARGET=all ./scripts/build.sh release
TARGET=all ./scripts/flash.sh --layout split --data-fs exfat /dev/sdX

# 写入 NTFS Data 分区布局，适合 Windows/大文件盘工作流
./scripts/flash.sh --layout split --data-fs ntfs /dev/sdX

# 写入 ext4 Data 分区布局，适合 Linux SSD/NVMe 工作流
./scripts/flash.sh --layout split --data-fs ext4 /dev/nvme0n1

# 写入 ext3 Data 分区布局，适合旧 Linux 兼容工作流
./scripts/flash.sh --layout split --data-fs ext3 /dev/nvme0n1

# 写入 UDF Data 分区布局，适合 Ventoy 风格兼容性验证
./scripts/flash.sh --layout split --data-fs udf /dev/sdX

# 写入 XFS Data 分区布局，适合 Linux SSD/NVMe 工作流
./scripts/flash.sh --layout split --data-fs xfs /dev/nvme0n1

# 显式指定 Ventoy 资产目录，安装 Windows WIMBOOT fallback 所需文件
./scripts/flash.sh --layout split --ventoy-assets ../Ventoy/INSTALL/ventoy /dev/sdX

# 先预览将要执行的分区/格式化命令
./scripts/flash.sh --dry-run --layout split /dev/sdX
```

`flash.sh` 默认使用 split GPT 布局：第 1 分区是 FAT32 ESP，只保存 NextBoot
启动文件；第 2 分区是 Data 分区，默认 exFAT，也可选择 FAT32、ext2/ext3/ext4、NTFS、UDF 或 XFS，用来存放
`/ISO` 下的 ISO/WIM/VHD 文件。旧式单分区 FAT32 仍可通过 `--layout single` 生成。
设置 `TARGET=all` 或 `--target all` 时，ESP 会同时安装 `EFI/BOOT/BOOTX64.EFI` 和
`EFI/BOOT/BOOTIA32.EFI`、`EFI/BOOT/BOOTAA64.EFI`，适合一块 SSD/U 盘/SD 卡跨 x86_64、IA32 与 AArch64 UEFI 固件启动。
在 Linux 上写入 ext/UDF/XFS Data 分区需要 `mkfs.ext2`/`mkfs.ext3`/`mkfs.ext4`/`mkudffs`/`mkfs.xfs`。在 macOS 上写入 NTFS
Data 分区需要额外安装 `mkfs.ntfs`/`mkntfs`；若要脚本自动创建 `/ISO` 目录，还需要可写
NTFS 驱动，例如 `ntfs-3g`。macOS 没有可靠内置 ext 写挂载，因此 `--data-fs ext2/ext3/ext4`
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
