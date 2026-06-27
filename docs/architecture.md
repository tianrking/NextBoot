# NextBoot 架构设计文档

## 系统架构概览

```
┌─────────────────────────────────────────────────────────────────────┐
│                         NextBoot UEFI Application                   │
├─────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │   Menu UI   │  │   Config    │  │   Cache     │  │    Log      │ │
│  │  (GOP/Text) │  │  (JSON)     │  │ (filelist)  │  │  (Serial)   │ │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └─────────────┘ │
│         │                │                │                         │
├─────────┴────────────────┴────────────────┴─────────────────────────┤
│                        Core Services Layer                           │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                    Virtual Block IO Driver                      ││
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          ││
│  │  │ LBA Mapping  │  │ CD-ROM Emul  │  │ Write Protect│          ││
│  │  └──────────────┘  └──────────────┘  └──────────────┘          ││
│  └─────────────────────────────────────────────────────────────────┘│
├─────────────────────────────────────────────────────────────────────┤
│                        File System Layer                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                 │
│  │ FAT32/exFAT │  │    NTFS     │  │   ISO9660   │                 │
│  │  (Data/ESP) │  │   (Data)    │  │   (ISO)     │                 │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                 │
├─────────┴────────────────┴────────────────┴─────────────────────────┤
│                        OS Boot Layer                                │
│  ┌───────────────────────────┐  ┌───────────────────────────┐      │
│  │      Linux Boot           │  │      Windows Boot         │      │
│  │  • Kernel/Initrd Loading  │  │  • Block IO Hijack        │      │
│  │  • Cmdline Injection      │  │  • ACPI Table Injection   │      │
│  │  • Multi-distro Support   │  │  • BCD Modification       │      │
│  └───────────────────────────┘  └───────────────────────────┘      │
├─────────────────────────────────────────────────────────────────────┤
│                        UEFI Services                                │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │  Block IO   │  │    GOP      │  │  SimpleTxt  │  │  Runtime    │ │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        Hardware Layer                               │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                    USB Mass Storage Device                   │   │
│  │  ┌──────────────┐  ┌──────────────────────────────────────┐ │   │
│  │  │  Partition 1 │  │         Partition 2 (Data)           │ │   │
│  │  │  ESP (FAT32) │  │ exFAT/NTFS - ISO/IMG/WIM/VHD Files   │ │   │
│  │  │  200MB       │  │  (Rest of disk)                      │ │   │
│  │  └──────────────┘  └──────────────────────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

## 核心模块设计

### 1. Virtual Block IO (核心)

这是实现"免格启动"的关键模块。

```rust
// 虚拟设备工作流程
1. 用户选择 ISO 文件
2. 获取 ISO 在物理设备上的位置 (起始 LBA, 大小)
3. 创建 VirtualBlockIo 实例
4. 向 UEFI 注册新的 Block IO Protocol
5. 声明设备类型为 CD-ROM 或 HDD
6. 后续读取请求被转换为物理读取
```

**LBA 映射算法:**
```
Virtual LBA 0 → Physical LBA (ISO_Start)
Virtual LBA N → Physical LBA (ISO_Start + N)
```

### 2. 文件系统层

**支持的文件系统:**

| 文件系统 | 用途 | 块大小 | 最大文件 |
|---------|------|--------|---------|
| FAT32 | ESP 分区 | 512B/4K | 4GB |
| exFAT | Data 分区 | 512B/4K | 无限制 |
| ext2/ext3/ext4 | Data 分区 | 4K QEMU 覆盖 | 无限制 |
| NTFS | Data 分区 | 512B/4K | 无限制 |
| UDF | Data 分区 | 512B/4K | 无限制 |
| XFS | Data 分区 | 4K QEMU 受限子集 | 无限制 |
| ISO9660 | ISO 镜像内部 | 2048B | 无限制 |

**ISO 文件扫描流程:**
```
1. 挂载或 raw BlockIO 扫描 Data 分区 (FAT32/exFAT/ext2/ext3/ext4/NTFS/UDF/XFS)
2. 遍历 /ISO 目录
3. 过滤扩展名: .iso, .img, .wim, .vhd
4. 读取文件信息: 名称、大小、起始 LBA
5. (可选) 读取 ISO 内部，检测 OS 类型
6. 缓存到 filelist.json
```

### 3. OS 引导层

**Linux 引导流程:**
```
1. 解析 ISO 内部结构
2. 定位 vmlinuz 和 initrd
3. 加载到内存
4. 构造内核命令行:
   - iso-scan/filename=/ISO/ubuntu.iso (Ubuntu)
   - findiso=/ISO/debian.iso (Debian)
5. 调用 Linux Kernel 启动协议
```

**Windows 引导流程 (复杂):**
```
1. 创建虚拟 CD-ROM 设备
2. 加载 bootmgfw.efi
3. (关键) 注入内存驻留驱动
4. 启动 bootmgfw.efi
5. Windows 内核接管后仍能找到虚拟设备
```

## 分区布局

**标准 GPT 分区表:**

| 分区 | 类型 | 文件系统 | 大小 | 内容 |
|------|------|---------|------|------|
| 1 | ESP | FAT32 | 200MB | Bootloader, Config |
| 2 | Basic Data | exFAT/NTFS | 剩余空间 | ISO 文件 |

**扇区对齐要求:**
- 必须动态读取设备的 `Block Size`
- 支持 512B 和 4K 扇区
- 所有 LBA 计算必须基于实际块大小

## 启动流程

```
Power On
    │
    ▼
UEFI Firmware
    │
    ▼
Load nextboot.efi from ESP
    │
    ▼
┌─────────────────────────────┐
│     NextBoot Main Flow      │
├─────────────────────────────┤
│ 1. Init UEFI Services       │
│ 2. Detect Storage Devices   │
│ 3. Find Data Partition      │
│ 4. Scan ISO Files           │
│ 5. Show Boot Menu           │
│ 6. User Selection           │
│ 7. Setup Virtual Block IO   │
│ 8. Boot Selected OS         │
└─────────────────────────────┘
    │
    ▼
Target OS (Windows/Linux)
```

## 内存布局

```
UEFI Memory Map:
┌────────────────────┐ 0xFFFFFFFF
│   UEFI Runtime     │
├────────────────────┤
│   Reserved         │
├────────────────────┤
│   Bootloader Code  │
├────────────────────┤
│   Kernel (Linux)   │
├────────────────────┤
│   Initrd           │
├────────────────────┤
│   Heap (Alloc)     │
├────────────────────┤
│   Stack            │
└────────────────────┘ 0x00000000
```

## 关键技术决策

### ADR-001: 使用 Rust 而非 C
- **决策**: 使用 Rust + uefi-rs
- **原因**: 内存安全、现代化工具链、零成本抽象
- **风险**: uefi-rs 生态较小

### ADR-002: 默认 exFAT，同时扩展 Data 文件系统
- **决策**: Data 分区默认使用 exFAT；raw BlockIO 扫描路径同时支持 FAT32/exFAT/ext2/ext3/ext4/NTFS/UDF/XFS
- **原因**: exFAT 更简单，适合作为默认写盘格式；NTFS 覆盖 Windows 用户和大文件盘的常见布局；ext2/3/4、UDF 与 XFS 覆盖 Linux SSD 与 Ventoy 风格数据盘
- **风险**: 某些固件不会暴露 NTFS/ext/UDF/XFS SimpleFS，因此这些格式依赖 NextBoot 自带只读解析器；macOS 无可靠内置 ext/XFS 写挂载；XFS 当前只覆盖 QEMU 受限目录子集

### ADR-003: 标准 GPT 而非混合分区
- **决策**: 严格使用标准 GPT
- **原因**: 避免 Ventoy 的兼容性问题
- **风险**: 无

## 测试策略

### QEMU + OVMF 测试

```bash
# 安装 OVMF (UEFI 固件)
sudo apt install ovmf

# 运行测试
qemu-system-x86_64 \
  -bios /usr/share/OVMF/OVMF_CODE.fd \
  -drive file=disk.img,format=raw \
  -m 2G \
  -serial stdio
```

### 实机测试矩阵

| 主板品牌 | UEFI 版本 | 4K 支持 | 状态 |
|---------|----------|---------|------|
| Dell | 2.x | Yes | TODO |
| Lenovo | 2.x | Yes | TODO |
| HP | 2.x | Yes | TODO |
| ASUS | 2.x | Yes | TODO |
| MSI | 2.x | Yes | TODO |
