# NextBoot MVP Progress

## Version: 0.1.0

### Status: HARDENING — release readiness is not yet established

## 2026-09-12: Reliable release work

- Product scope confirmed: UEFI multi-image installation and recovery media,
  prioritizing x86_64 mainstream workflows and retaining separate evidence for
  other architectures. Runtime compatibility assets may be reused with their
  provenance, checksums, and licenses recorded.
- Completed: update partition discovery now uses GPT types and volume metadata
  instead of fixed partition numbers. Release and older layouts are covered;
  ambiguous disks and invalid ESP filesystems fail before mounting. Force cannot
  waive ESP validation. Dry runs inspect the actual inventory.
- Validation: 10 partition/parser and shell orchestration tests pass locally,
  with additional subcases for both layouts and unsafe inventories. Fixtures are
  isolated from build output and no physical disk was written.
- In progress: release/runtime parity, full QEMU failure diagnosis, and evidence
  for real OS workflows. Hardware qualification remains outstanding.
- Runtime parity implemented: release builds now include eight SHA256-pinned
  upstream resources, original license notices, patch attribution, and resulting
  hashes. Fifty files are verified inside the actual disk after generation.
  Real ISO QA uses that same release builder. The explicit `--without-runtime`
  option is only for developer fixtures.
- Local Rust host tests and x86_64 release build pass. The first updater commit
  also passes Linux CI, all three architecture checks, and default QEMU smoke.
- QEMU log capture now supports Windows pipes; marker fragmentation, input
  handshake, EOF failure, and timeout termination checks pass locally.
- USB 4K failure reproduced with QEMU 8.2 and Ubuntu OVMF 2024.02: the legacy
  usb-storage wrapper ignored block-size properties. The identical image boots
  into NextBoot when the USB BOT device has an explicitly configured scsi-hd
  child. The full matrix now uses that topology for USB 4K. Upstream diagnosis:
  https://github.com/qemu/qemu/commit/3089637461693837cafd2709ef36d0cf6a4a8ed8
- Historical completion checkboxes below describe implementation, not certified
  compatibility or production release readiness.
- Growth correction: preserve the partial-cluster tail in exFAT VolumeLength;
  reject shrink attempts before writes and make repeated desktop growth a no-op.
  Three host geometry tests and generated-image preservation checks pass.
  The rebuilt release loader passed QEMU at 256 MiB, grew the same image to
  512 MiB, and reported no further growth on reboot. All fifty bundled runtime
  and notice files still verify after firmware growth.
- Full QEMU boot matrix passed on commit 7b8fe81. Its image-generation assertions
  were subsequently aligned with the corrected USB BOT topology. Alpine 3.24.1
  reached its login prompt through the actual release-media builder locally.
- Build inputs are now pinned to Rust 1.98.1 and the checked-in Cargo.lock;
  build/check and host validation use --locked. The release build and 68 tests
  across config, filesystem, image and Linux libraries pass with that lockfile.
  This controls dependency drift; byte-for-byte reproducibility is not claimed.
- Returning-loader regression reproduced and fixed: retain firmware-owned
  LoadedImage.FilePath instead of substituting a Rust allocation that both
  firmware and Rust would free. A returning Linux EFI fixture now reaches parent
  success and initrd protocol cleanup without panic. Linux smoke cases wait for
  that cleanup marker, beyond the earlier handoff-only assertion.
- Failed-image recovery implemented: a broken ISO now returns to the menu and a
  different Linux ISO can start in the same session. Both attempts' virtual
  devices, initrd provider and runtime metadata were verified released in QEMU.
  The second menu disables the automatic boot timeout. Firmware cleanup refusal
  stops retries and retains referenced allocations. The recovery test also found
  and fixed conflicting ownership between preinstalled DiskIo and OVMF's adapter;
  firmware now owns its DiskIo adapter on NextBoot's BlockIO device.
- Validation for recovery: x86_64 release build, 33 virtual-I/O host tests, seven
  QEMU-runner checks, source health, and the two-image QEMU recovery workflow pass
  locally. Full-matrix and cross-architecture CI must be rechecked for this change.
- The recovery implementation now passes the full QEMU matrix and two-image
  retry test in CI at fa310d0, along with all seven basic checks. Alpine login,
  Kali language selection and Ubuntu serial-mode selection all pass through
  actual release media. All eleven checks pass on that commit.
- Real ISO jobs run independently in both PR and scheduled/main workflows.
  Boot evidence now includes source state, ISO/loader/firmware hashes, VM
  configuration, markers, result and log digest. Failed launches cannot reuse
  a stale serial log. Ubuntu VM resources are aligned with a successful direct
  official-ISO control; local Windows-hosted verification also reached the
  installer with the larger VM. Alpine evidence recording and failed-launch
  recording/stale-log rejection were verified locally. Main-branch pushes now
  run the full matrix and recovery check as well as independent real ISO jobs.

## 完成的工作

### 2026-09-12: Recoverable updating

- The mounted-volume backend now stages and verifies each new file, backs up
  existing bytes, writes a pending journal, and restores original files after
  write errors. Explicit recovery works after process interruption without
  requiring builds or downloads. Intact backups are retained after completion.
- Runtime migration uses the same pinned 50-file bundle as the release builder.
  Only the fallback loaders and explicit runtime paths can be changed; ISO,
  configuration, unrelated files, and externally modified targets are protected.
- Linux/macOS frontend integration now mounts DATA for runtime updates, supports
  rollback and loader-only mode, and cleans up mounts it created after errors.
  Linux uses a unique temporary mount directory. macOS preserves pre-existing
  mounts and reads mount paths from diskutil's structured output.
- Local evidence: 18 transaction/loader test cases (the POSIX symlink case is
  skipped on Windows), 13 partition/frontend plan cases, and actual-loader plus
  50-file runtime migration/no-op/rollback integration. Linux CI covers real
  FAT/exFAT loopback filesystems before and after updates. Windows CI covers
  stable disk/volume selection plus a fresh native VHD update, no-op, rollback,
  detach and remount.
- Physical host validation and power-loss behavior remain outstanding. This
  implementation does not claim cross-volume atomicity.

### Phase 1: 项目设置 ✅
- [x] 创建 workspace 结构
- [x] 配置 UEFI target
- [x] 设置依赖项
- [x] 创建构建脚本

### Phase 2: 文件系统支持 ✅
- [x] FAT32 读取支持
- [x] exFAT 读取支持  
- [x] ISO9660 读取支持
- [x] GPT 分区解析
- [x] 动态块大小检测

### Phase 3: 虚拟 Block IO ✅
- [x] LBA 映射实现
- [x] 虚拟设备配置
- [x] Protocol 定义
- [x] 只读保护

### Phase 4: 菜单系统 ✅
- [x] 控制台文本渲染
- [x] GOP 图形支持
- [x] 键盘输入处理
- [x] 菜单状态管理
- [x] 主题系统

### Phase 5: Linux 引导支持 ✅
- [x] 发行版检测
- [x] Kernel 加载
- [x] Initrd 加载
- [x] 启动配置
- [x] 命令行参数注入

### Phase 6: Windows 引导支持 ✅
- [x] PE 文件解析
- [x] Boot manager 加载
- [x] 配置结构
- [x] ACPI 表定义
- [x] BCD 解析

### Phase 7: 主程序集成 ✅
- [x] 设备检测
- [x] ISO 扫描
- [x] 启动流程
- [x] 错误处理

## 当前状态

项目代码已基本完成，正在进行编译测试和错误修复。

主要剩余工作：
1. 修复 UEFI API 兼容性问题
2. 完善帧缓冲区操作
3. 实现 UEFI Protocol 注册
4. 实际硬件测试

## 文件清单

### 核心模块
- `crates/nextboot-boot/src/main.rs` - 主入口
- `crates/nextboot-boot/src/init.rs` - 初始化
- `crates/nextboot-boot/src/scanner.rs` - ISO 扫描
- `crates/nextboot-boot/src/boot.rs` - 引导管理

### 文件系统
- `crates/nextboot-fs/src/lib.rs` - 核心接口
- `crates/nextboot-fs/src/fat32.rs` - FAT32 实现
- `crates/nextboot-fs/src/exfat.rs` - exFAT 实现
- `crates/nextboot-fs/src/iso9660.rs` - ISO9660 实现
- `crates/nextboot-fs/src/gpt.rs` - GPT 解析

### 虚拟设备
- `crates/nextboot-virtio/src/lib.rs` - 虚拟 Block IO
- `crates/nextboot-virtio/src/mapping.rs` - LBA 映射
- `crates/nextboot-virtio/src/protocol.rs` - UEFI 协议

### 菜单界面
- `crates/nextboot-menu/src/lib.rs` - 菜单核心
- `crates/nextboot-menu/src/gop.rs` - GOP 图形
- `crates/nextboot-menu/src/console.rs` - 控制台
- `crates/nextboot-menu/src/menu.rs` - 菜单渲染

### 操作系统引导
- `crates/nextboot-linux/src/lib.rs` - Linux 引导
- `crates/nextboot-windows/src/lib.rs` - Windows 引导

### 脚本
- `scripts/build.sh` - 构建脚本
- `scripts/run-qemu.sh` - QEMU 测试
- `scripts/flash.sh` - 写入 U 盘

## 下一步计划

1. **完成编译修复** - 解决剩余的 API 兼容性问题
2. **QEMU 测试** - 在模拟环境中验证功能
3. **实机测试** - 在真实 UEFI 硬件上测试
4. **ISO 兼容性** - 测试各种 Linux 发行版 ISO
5. **文档完善** - 添加更多使用说明

## 已知限制

1. Windows 引导需要额外的内存持久化工作
2. 某些 UEFI 固件可能有兼容性问题
3. 4K Native 设备需要额外测试
