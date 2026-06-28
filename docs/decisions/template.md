# ADR 模板

## 编号规则
- 使用 4 位数字: ADR-0001, ADR-0002, ...
- 按时间顺序递增

## 文件命名
`docs/decisions/NNNN-short-title.md`

---

# ADR-NNNN: 决策标题

## Status
{Proposed | Accepted | Deprecated | Superseded by ADR-XXXX}

## Context
描述背景和需要解决的问题。
- 为什么需要做这个决策？
- 有什么约束条件？
- 相关的技术背景

## Decision
描述做出的决策。
- 我们决定做什么？
- 具体的技术方案是什么？

## Consequences
### 正面影响
- ...

### 负面影响
- ...

### 风险
- ...

## Alternatives Considered
### 方案 A: 名称
- 描述
- 优点
- 缺点
- 不选择的原因

### 方案 B: 名称
- 描述
- 优点
- 缺点
- 不选择的原因

## Related
- 相关的需求章节
- 相关的其他 ADR
- 参考资料

---

## 示例

# ADR-0001: 使用 Rust 作为开发语言

## Status
Accepted

## Context
NextBoot 是一个 UEFI 启动加载器，需要在 UEFI 环境中运行。传统的 UEFI 开发使用 C 语言和 EDK2 框架。

约束条件:
- 必须支持 no_std 环境
- 需要与 UEFI C API 交互
- 代码安全性要求高 (启动加载器崩溃会导致系统无法启动)

## Decision
使用 Rust 语言开发 NextBoot，配合 uefi-rs 库。

## Consequences
### 正面影响
- 内存安全保证，减少运行时错误
- 现代化的包管理和构建系统
- 零成本抽象，不牺牲性能

### 负面影响
- uefi-rs 生态系统相对较小
- 团队需要学习 Rust
- 调试可能比 C 更困难

### 风险
- uefi-rs 可能有未发现的 bug
- 某些 UEFI 功能可能尚未支持

## Alternatives Considered
### 方案 A: C + EDK2
- 行业标准，文档丰富
- 成熟稳定
- 但缺乏内存安全保证

### 方案 B: C++ + gnu-efi
- 更轻量级
- 但 C++ 在 no_std 环境支持有限

## Related
- Requirements Section 3: 技术栈
- uefi-rs: https://github.com/rust-osdev/uefi-rs
