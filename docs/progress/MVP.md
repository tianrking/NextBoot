# MVP 开发进度

## 目标
实现最小可行性版本，能够启动 Ubuntu ISO。

## 阶段概览

```
┌─────────────────────────────────────────────────────────────────┐
│                        MVP Roadmap                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Phase 1: 基础设施 ████████░░░░░░░░░░░░  40%                    │
│  ├─ 项目骨架       ████████████████████  DONE                   │
│  ├─ UEFI 入口      ████░░░░░░░░░░░░░░░░  TODO                   │
│  ├─ GPT 解析       ████████████████████  DONE (骨架)            │
│  └─ FAT32 读取     ████░░░░░░░░░░░░░░░░  TODO                   │
│                                                                 │
│  Phase 2: 核心功能 ░░░░░░░░░░░░░░░░░░░░  0%                     │
│  ├─ exFAT 读取     ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  ├─ ISO 扫描       ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  └─ 文件缓存       ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│                                                                 │
│  Phase 3: 虚拟化   ░░░░░░░░░░░░░░░░░░░░  0%                     │
│  ├─ VirtIO 驱动    ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  ├─ LBA 映射       ████████████████████  DONE (骨架)            │
│  └─ Protocol 注册  ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│                                                                 │
│  Phase 4: Linux    ░░░░░░░░░░░░░░░░░░░░  0%                     │
│  ├─ Kernel 加载    ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  ├─ Initrd 加载    ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  └─ Ubuntu 启动    ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│                                                                 │
│  Phase 5: UI       ░░░░░░░░░░░░░░░░░░░░  0%                     │
│  ├─ 文本菜单       ████████████████████  DONE (骨架)            │
│  ├─ 键盘交互       ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│  └─ 状态显示       ░░░░░░░░░░░░░░░░░░░░  TODO                   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## 详细任务列表

### Phase 1: 基础设施

#### Task: 项目骨架搭建
- **Owner**: architect
- **Status**: DONE
- **Started**: 2026-03-05
- **Completed**: 2026-03-05
- **Changes**:
  - 创建 Cargo workspace 配置
  - 创建 6 个 crate 骨架
  - 创建 AI 协同规范文档
- **Decisions**:
  - 使用 uefi-rs 作为 UEFI 绑定
  - 模块化设计，每个功能一个 crate

#### Task: UEFI 入口实现
- **Owner**: uefi-dev
- **Status**: TODO
- **Priority**: P0
- **Dependencies**: 无
- **Description**: 实现完整的 UEFI 入口点和服务初始化
- **Acceptance Criteria**:
  - [ ] 能在 QEMU + OVMF 中启动
  - [ ] 能输出日志到串口
  - [ ] 能检测存储设备

#### Task: FAT32 读取实现
- **Owner**: fs-dev
- **Status**: IN_PROGRESS
- **Priority**: P0
- **Dependencies**: UEFI 入口
- **Description**: 实现 FAT32 只读文件系统
- **Notes**: 需要支持长文件名 (LFN)

---

### Phase 2: 核心功能

#### Task: exFAT 读取实现
- **Owner**: fs-dev
- **Status**: TODO
- **Priority**: P0
- **Dependencies**: FAT32 完成
- **Description**: 实现 exFAT 只读文件系统
- **Notes**: 这是 Data 分区的主要文件系统

#### Task: ISO 文件扫描
- **Owner**: fs-dev
- **Status**: TODO
- **Priority**: P0
- **Dependencies**: exFAT 完成
- **Description**: 递归扫描 /ISO 目录
- **Acceptance Criteria**:
  - [ ] 支持 .iso, .img, .wim, .vhd
  - [ ] 记录文件起始 LBA
  - [ ] 计算文件大小

---

### Phase 3: 虚拟化

#### Task: 虚拟 Block IO 驱动
- **Owner**: uefi-dev
- **Status**: TODO
- **Priority**: P0
- **Dependencies**: ISO 扫描
- **Description**: 实现虚拟 Block IO Protocol
- **Notes**: 这是核心功能，需要仔细测试

---

### Phase 4: Linux 引导

#### Task: Ubuntu 启动支持
- **Owner**: os-dev
- **Status**: TODO
- **Priority**: P0
- **Dependencies**: VirtIO 驱动
- **Description**: 实现 Ubuntu ISO 启动
- **Acceptance Criteria**:
  - [ ] 能加载 vmlinuz
  - [ ] 能加载 initrd
  - [ ] 能注入 iso-scan 参数
  - [ ] Ubuntu Desktop 能正常启动

---

## 阻塞问题

(目前无阻塞)

## 风险

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| uefi-rs API 变更 | 高 | 锁定版本，定期更新 |
| QEMU 测试环境 | 中 | 文档化安装步骤 |
| 4K 扇区兼容性 | 高 | 早期测试 4K 设备 |

## 里程碑

- [ ] **M1**: 能在 QEMU 启动并输出日志 (Week 1)
- [ ] **M2**: 能读取 exFAT 分区 (Week 2)
- [ ] **M3**: 能扫描并列出 ISO 文件 (Week 3)
- [ ] **M4**: 能在 QEMU 启动 Ubuntu (Week 4)
- [ ] **M5**: 实机测试通过 (Week 5)
