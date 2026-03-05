# NextBoot 架构设计文档

## 概述

NextBoot 是一个基于 Rust 的 UEFI 启动加载器，核心功能是无需格式化 U 盘，通过拖入 ISO 文件并模拟虚拟光驱来启动操作系统。

## 系统架构

```
┌─────────────────────────────────────────────────────────────┐
│                      UEFI Firmware                          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    nextboot-boot (Main)                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │   init   │  │ scanner  │  │   boot   │  │   main   │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
        │               │               │
        ▼               ▼               ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐
│ nextboot-fs │ │nextboot-menu│ │nextboot-virt│
│             │ │             │ │     io      │
│ ┌─────────┐ │ │ ┌─────────┐ │ │ ┌─────────┐ │
│ │  FAT32  │ │ │ │   GOP   │ │ │ │ mapping │ │
│ │  exFAT  │ │ │ │ console │ │ │ │protocol │ │
│ │ ISO9660 │ │ │ │  menu   │ │ │ │  vbio   │ │
│ │   GPT   │ │ │ └─────────┘ │ │ └─────────┘ │
│ └─────────┘ │ └─────────────┘ └─────────────┘
└─────────────┘
        │
        ▼
┌─────────────────────────────────────────────┐
│           nextboot-linux / nextboot-windows │
│                                             │
│  ┌───────────┐     ┌───────────────────┐   │
│  │  Linux    │     │     Windows       │   │
│  │  Kernel   │     │    Boot Manager   │   │
│  │  Loader   │     │                   │   │
│  └───────────┘     └───────────────────┘   │
└─────────────────────────────────────────────┘
```

## 模块说明

### 1. nextboot-boot (主程序)

主入口点，协调所有模块的工作。

**职责：**
- UEFI 服务初始化
- 存储设备检测
- ISO 文件扫描
- 启动流程控制

**关键文件：**
- `main.rs` - UEFI 入口点
- `init.rs` - 设备检测和初始化
- `scanner.rs` - ISO 文件扫描
- `boot.rs` - 引导管理

### 2. nextboot-fs (文件系统)

提供各种文件系统的只读支持。

**支持的文件系统：**
- FAT32 (ESP 分区)
- exFAT (数据分区)
- ISO9660 (ISO 镜像)

**关键 trait:**
```rust
pub trait FileSystem: Sized {
    const FS_TYPE: FileSystemType;
    fn init(block_io: &dyn BlockIoOps) -> Result<Self, FsError>;
    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError>;
    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    fn stat(&self, path: &str) -> Result<FileInfo, FsError>;
    fn block_size(&self) -> u32;
}
```

### 3. nextboot-virtio (虚拟 Block IO)

实现 ISO 文件到虚拟设备的映射。

**核心概念：**
- **LBA Mapping**: 虚拟 LBA 到物理 LBA 的转换
- **Virtual Device**: 模拟的 CD-ROM 或硬盘设备
- **Block IO Protocol**: UEFI 兼容的协议实现

**工作流程：**
```
用户选择 ISO → 创建虚拟设备配置 → 建立 LBA 映射表 → 注册到 UEFI
```

### 4. nextboot-menu (菜单界面)

提供用户交互界面。

**支持：**
- 文本模式菜单
- GOP 图形模式
- 键盘输入处理

### 5. nextboot-linux (Linux 引导)

支持各种 Linux 发行版的启动。

**支持的发行版：**
- Ubuntu
- Debian
- Fedora
- Arch Linux
- 通用 Linux

**启动流程：**
```
检测发行版 → 定位 Kernel/Initrd → 加载到内存 → 设置 cmdline → 启动
```

### 6. nextboot-windows (Windows 引导)

支持 Windows ISO 的启动。

**挑战：**
- Windows 启动后会重置 USB 驱动
- 需要虚拟设备在 Windows 内核接管后仍然可见

**解决方案：**
- 使用 CD-ROM 类型虚拟设备
- 可能需要 ACPI 表注入

## 数据流

```
1. 启动阶段
   UEFI → nextboot-boot.efi → 初始化服务

2. 扫描阶段
   检测设备 → 打开文件系统 → 扫描 ISO 文件

3. 选择阶段
   显示菜单 → 用户选择 → 确定启动类型

4. 准备阶段
   创建虚拟设备 → 加载引导文件 → 设置参数

5. 启动阶段
   跳转到 OS 引导程序 → 控制权转移
```

## 内存布局

```
┌────────────────────────────────────┐ 0xFFFFFFFF
│         UEFI Runtime Services      │
├────────────────────────────────────┤
│         UEFI Boot Services         │
├────────────────────────────────────┤
│         NextBoot Code/Data         │
├────────────────────────────────────┤
│         ISO File Cache             │
├────────────────────────────────────┤
│         Kernel/Initrd              │
├────────────────────────────────────┤
│         Heap (Pool Allocator)      │
├────────────────────────────────────┤
│         Stack                      │
└────────────────────────────────────┘ 0x0
```

## 错误处理

所有模块使用 `Result<T, E>` 进行错误处理：

- `FsError` - 文件系统错误
- `VirtIoError` - 虚拟设备错误
- `LinuxBootError` - Linux 启动错误
- `WindowsBootError` - Windows 启动错误

## 安全考虑

1. **只读操作**: 所有文件系统操作都是只读的
2. **边界检查**: 所有数组访问都有边界检查
3. **内存安全**: Rust 保证内存安全
4. **无动态链接**: 所有代码静态链接

## 性能优化

1. **零拷贝**: 尽量避免数据复制
2. **缓存**: FAT 表和目录信息缓存
3. **按需加载**: 只加载需要的文件
4. **最小分配**: 减少内存分配次数

## 测试策略

1. **QEMU + OVMF**: 模拟 UEFI 环境
2. **单元测试**: 纯逻辑模块
3. **集成测试**: 完整启动流程
4. **实机测试**: 多种硬件平台

## 扩展性

系统设计支持：
- 添加新的文件系统支持
- 添加新的操作系统引导
- 自定义主题和界面
- 插件系统（未来）
