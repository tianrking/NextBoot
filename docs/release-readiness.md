# Release readiness / 发布验收

Status: **hardening; no new release certified**. Updated 2026-09-12.

NextBoot targets UEFI multi-image installation and recovery media. The first
reliability target is x86_64 with Secure Boot disabled. IA32 and AArch64 builds
remain available, with separate compatibility evidence. Legacy BIOS is outside
this product scope. Public Secure Boot distribution is a later milestone.

当前目标是可靠的 UEFI 多镜像装机与恢复介质，先完成 x86_64 的真实使用流程。
编译通过、合成镜像通过、进入系统安装界面、完成安装和真实硬件验证分别记录。
任何一层通过都不能代替后续层级。

## Existing release versus current development

The inspected v0.0.3 public image contains the fallback EFI loaders and an empty
`/ISO` directory, but omits the compatibility runtime used by earlier real-ISO
tests. It is a development baseline, not the verified complete product described
by the target user flow. The changes below belong to the current main branch and
must not be attributed to that existing downloadable release.

v0.0.3 的发布介质缺少此前真实 ISO 测试所使用的兼容运行资源。当前修复尚未发布，
不能把主分支的验证结果当作 v0.0.3 的能力证明。

## Acceptance ledger

| Area | Evidence completed | Remaining acceptance |
| --- | --- | --- |
| Media contents | Builder includes SHA256-pinned runtime, notices and provenance; all 50 files verified in actual exFAT media | Final multi-architecture release artifact verification |
| Growth | Host geometry tests, unchanged/rejected-shrink image hashes, QEMU 256 → 512 MiB growth, second boot no-op, runtime preservation | Real device capacity and host remount checks |
| Partition discovery | Both GPT entry orders and invalid/ambiguous inventory tests; Linux CI; native Windows selection rejects system/boot disks and identifies a GPT ESP plus NEXTDATA by stable volume GUID | Physical Linux/macOS update checks and physical Windows USB update |
| Update data preservation | Journaled backup/replacement/recovery and conflict rejection; actual EFI + 50-file runtime migration/no-op/rollback; Linux FAT/exFAT post-update filesystem checks; native Windows disposable-VHD update/no-op/rollback/remount check passed in CI at d4a2b93 | Physical host update and power-interruption testing, backup retention management |
| Build inputs | Rust 1.98.1 and Cargo.lock pinned, --locked builds/tests | Final release CI and build provenance |
| Synthetic boot paths | Full QEMU matrix and two-image recovery passed at fa310d0 | Re-run against final release commit; synthetic Btrfs fixtures are not ordinary Btrfs support |
| Alpine 3.24.1 x64 | Actual release builder → Linux login prompt in QEMU | Physical machine and installation workflow |
| Ubuntu 26.04 Server x64 | Actual release builder → installer serial-mode selection in CI at fa310d0 | Complete installation and physical machine verification |
| Kali 2026.2 netinst x64 | Actual release builder → language selection in CI at fa310d0 | Complete installation and physical machine verification |
| Windows updater | Native disk selector and recoverable update flow passed on a disposable FAT32/exFAT VHD; VHD is detached and remounted after rollback | Physical USB update and power-interruption checks |
| Windows and WinPE boot | Code paths and synthetic tests exist | Official image installation/recovery checks |
| Other mainstream images | Families listed in ISO compatibility matrix | Exact version/hash/result rows, including rescue and appliance images |
| Failed image recovery | Broken ISO → menu → different Linux ISO passed in QEMU, including both attempts' resource cleanup; retry timeout disabled | Broad firmware/real-image regression coverage and cleanup-refusal behavior on hardware |
| Real hardware | Report tooling exists; public CSV has zero rows | Genuine device reports; no synthetic rows counted |
| Documentation | Current limitations and runtime notices documented | Final user guide, troubleshooting, update/rollback, release notes and checksums synchronized to exact release |

## Release gate

A candidate is not promoted just because it builds. Before publishing a usable
release, verify the final commit and final downloadable artifacts, not an earlier
commit or a disk with additional test-only assets. Record:

1. A clean source commit, Rust/dependency versions and passing required CI.
2. Downloadable image hashes, embedded loaders, runtime checksums, original
   license notices, pinned upstream source and matching NextBoot source archive.
3. Exact tested OS image versions/hashes, how far each reached, media geometry,
   firmware and whether the result came from QEMU or a physical machine.
4. Installation and update instructions, data-preservation/rollback checks,
   known limitations and troubleshooting that match the tested artifact.
5. Hardware coverage appropriate to every public compatibility claim.

## Continuation order

Finish real ISO validation and failed-boot recovery, complete safe updating with
runtime migration, broaden the mainstream OS evidence, then run the final media
and documentation audit. Make a separate commit for each verified milestone.

Related: [ISO matrix](iso-compatibility-matrix.md),
[hardware evidence](hardware/hardware-matrix-status.md),
[release media](release-media.md), [progress](progress/MVP.md).
