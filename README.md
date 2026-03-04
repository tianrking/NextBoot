# NextBoot - 新 Agent 入口指南

> **给新 Agent**: 只需阅读本文件即可掌握项目全貌并开始开发。

---

## 一句话概述

**NextBoot 是一个 Rust 编写的 UEFI 启动加载器，实现"免格启动"——不格式化 U 盘，直接拖入 ISO 文件就能启动操作系统。**

---

## 核心原理图

```
┌─────────────────────────────────────────────────────────────────┐
│                         用户视角                                 │
│   1. 把 nextboot.efi 复制到 U 盘 ESP 分区                       │
│   2. 把 xxx.iso 复制到 U 盘 Data 分区                           │
│   3. 从 U 盘启动 → 看到菜单 → 选择 ISO → 启动系统               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                       技术实现                                   │
│                                                                  │
│   ┌──────────────┐      ┌──────────────┐      ┌──────────────┐  │
│   │ 扫描 ISO 文件 │ ──▶ │ 创建虚拟设备  │ ──▶ │ 启动操作系统 │  │
│   │ (exFAT 分区) │      │ (Block IO)   │      │ (Linux/Win)  │  │
│   └──────────────┘      └──────────────┘      └──────────────┘  │
│          │                     │                     │          │
│          ▼                     ▼                     ▼          │
│   nextboot-fs            nextboot-virtio       nextboot-linux   │
│                          (核心模块!)            nextboot-windows │
└─────────────────────────────────────────────────────────────────┘
```

---

## 项目文件导航

### 📋 必读文件（按优先级）

| 文件 | 内容 | 何时阅读 |
|------|------|---------|
| **本文件** | 项目入口指南 | 第一个 |
| [PRD.md](PRD.md) | 产品需求，功能定义 | 了解需求时 |
| [docs/architecture.md](docs/architecture.md) | 详细架构设计 | 设计模块时 |
| [docs/progress/MVP.md](docs/progress/MVP.md) | 当前开发进度 | 开始任务前 |
| [AGENTS.md](AGENTS.md) | Agent 协同规范 | 多 Agent 协作时 |

### 📁 目录结构速览

```
NextBoot/
├── crates/                    # 所有代码在这里
│   ├── nextboot-boot/        # 🚪 入口点 (从这里开始)
│   ├── nextboot-fs/          # 📁 文件系统 (FAT32/exFAT/ISO9660)
│   ├── nextboot-virtio/      # 🔧 核心虚拟化 (最重要!)
│   ├── nextboot-menu/        # 🖥️ 菜单界面
│   ├── nextboot-linux/       # 🐧 Linux 引导
│   └── nextboot-windows/     # 🪟 Windows 引导
│
├── docs/
│   ├── architecture.md       # 架构文档
│   ├── progress/MVP.md       # 进度追踪
│   └── decisions/            # 技术决策记录
│
└── scripts/                  # 构建/测试脚本
    ├── build.sh              # 编译
    ├── run-qemu.sh           # QEMU 测试
    └── flash.sh              # 写入 U 盘
```

---

## 技术栈速查

| 技术 | 用途 | 备注 |
|------|------|------|
| **Rust** | 主语言 | `no_std` 环境 |
| **uefi-rs** | UEFI 绑定 | 主要依赖 |
| **x86_64-unknown-uefi** | 编译目标 | UEFI 应用 |

### 关键约束

```rust
// ❌ 不能用
std::*           // 无标准库
Vec::new()       // 需要先初始化分配器
println!()       // 使用 uefi 输出

// ✅ 可以用
alloc::vec::Vec  // 需要 extern crate alloc
uefi::println!   // UEFI 输出
log::info!       // 日志框架
```

---

## 模块依赖关系

```
nextboot-boot (入口)
    │
    ├── nextboot-fs (文件系统)
    │       │
    │       └── 读取 ISO 文件
    │
    ├── nextboot-virtio (虚拟设备)
    │       │
    │       └── 将 ISO 映射为虚拟 Block IO
    │
    ├── nextboot-menu (界面)
    │       │
    │       └── 用户选择 ISO
    │
    └── nextboot-linux / nextboot-windows
            │
            └── 启动选中的操作系统
```

---

## 当前状态 (MVP 阶段)

### 已完成 ✅
- [x] 项目骨架
- [x] 模块接口设计
- [x] GPT 解析骨架
- [x] LBA 映射骨架
- [x] 菜单骨架

### 进行中 🔄
- [ ] UEFI 入口实现
- [ ] FAT32 读取

### 待开始 ⏳
- [ ] exFAT 读取
- [ ] ISO 扫描
- [ ] 虚拟 Block IO
- [ ] Ubuntu 启动

---

## Agent 角色与任务

| Agent | 负责模块 | 当前优先任务 |
|-------|---------|-------------|
| `uefi-dev` | boot, virtio | 实现 UEFI 入口 |
| `fs-dev` | fs | 实现 FAT32/exFAT 读取 |
| `gui-dev` | menu | 实现文本菜单 |
| `os-dev` | linux, windows | 实现 Ubuntu 启动 |

---

## 快速开始命令

```bash
# 1. 构建
./scripts/build.sh release

# 2. 测试 (需要安装 OVMF)
./scripts/run-qemu.sh

# 3. 写入 U 盘
./scripts/flash.sh /dev/sdX
```

---

## 关键代码位置

### 想了解...看这里：

| 我想了解... | 看这个文件 |
|------------|-----------|
| 程序入口点 | [crates/nextboot-boot/src/main.rs](../crates/nextboot-boot/src/main.rs) |
| 如何读取文件 | [crates/nextboot-fs/src/lib.rs](../crates/nextboot-fs/src/lib.rs) |
| 虚拟设备原理 | [crates/nextboot-virtio/src/lib.rs](../crates/nextboot-virtio/src/lib.rs) |
| 如何启动 Linux | [crates/nextboot-linux/src/lib.rs](../crates/nextboot-linux/src/lib.rs) |
| 分区表解析 | [crates/nextboot-fs/src/gpt.rs](../crates/nextboot-fs/src/gpt.rs) |

---

## 常见问题

### Q: 为什么选择 Rust 而不是 C？
A: 内存安全。启动加载器崩溃会导致系统无法启动，Rust 的安全保证很重要。

### Q: 为什么用 exFAT 而不是 NTFS？
A: exFAT 更简单，UEFI 支持更好，且支持 >4GB 文件。

### Q: Windows 启动为什么难？
A: Windows 启动后会重置 USB 驱动，虚拟设备会丢失。需要特殊处理（内存补丁/ACPI 注入）。

### Q: 什么是 Block IO 劫持？
A: 拦截 UEFI 的磁盘读取请求，将虚拟 LBA 转换为 ISO 文件在物理设备上的位置。

---

## 开始工作流程

```
1. 阅读 docs/progress/MVP.md 了解当前进度
2. 找到你负责的模块
3. 在 MVP.md 中将任务状态改为 IN_PROGRESS
4. 编写代码
5. 完成后将状态改为 DONE
6. 记录关键决策
```

---

## 联系与资源

- **UEFI 规范**: https://uefi.org/specifications
- **uefi-rs 文档**: https://docs.rs/uefi
- **OSDev Wiki**: https://wiki.osdev.org/UEFI

---

*最后更新: 2026-03-05*
