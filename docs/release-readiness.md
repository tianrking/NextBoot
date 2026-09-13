# Release readiness / 发布验收

Status: **v0.1.0-rc.14 prerelease**. Updated 2026-09-13. The published release gate mounted a same-size 7GB image through the native Windows exFAT driver, verified writing and reading on that mounted volume, and performed full-range write verification through a fixed VHD wrapper. This is not a stable or physical-hardware certification.

Physical Intel UEFI evidence on 2026-09-13 confirmed first-boot GPT/exFAT growth on removable SD media, then exposed a raw fallback scan that continued into internal disks when firmware supplied no SimpleFileSystem handles for NEXTDATA. RC.14 keeps the disk-identity filter and adds a conservative parent-device-path fallback: if a firmware omits a stable identity, only sibling handles under the boot device are considered and unknown devices are skipped. Physical menu and ISO handoff remain required before stable promotion.

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
by the target user flow. The current downloadable candidate is
`v0.1.0-rc.14`; it must not be interpreted as completed-installation or
physical-hardware certification.

v0.0.3 的发布介质缺少此前真实 ISO 测试所使用的兼容运行资源。`v0.1.0-rc.14`
以带 QEMU 证据的预发布形式发布这些修复；不能把它当作完成安装或真实硬件认证。

## Acceptance ledger

| Area | Evidence completed | Remaining acceptance |
| --- | --- | --- |
| Media contents | RC.13 release workflow built all three fallback loaders, verified the release media/runtime, published the six downloadable assets, and verified every uploaded asset name | Physical-download recheck and real multi-architecture firmware evidence |
| Windows exFAT mount and write | A physical SD-card report showed RAW. RC.8 blocks publication until Windows natively mounts a generated 7GB image as exFAT, finds `/ISO`, writes and reads a probe file, then invokes the Windows writer's full-range comparison through the mounted fixed VHD. The writer is attached to the release so users can verify an actual physical write instead of trusting a flasher completion message. | Run the attached writer on Windows physical removable media and record its pass result before stable promotion |
| Growth | Host geometry tests, unchanged/rejected-shrink image hashes, QEMU firmware expansion and post-growth exFAT verification; firmware updates Allocation Bitmap `DataLength` before publishing the new boot geometry | Real device capacity and host remount checks |
| Partition discovery | Both GPT entry orders and invalid/ambiguous inventory tests; Linux CI; native Windows selection rejects system/boot disks and identifies a GPT ESP plus NEXTDATA by stable volume GUID | Physical Linux/macOS update checks and physical Windows USB update |
| Update data preservation | Journaled backup/replacement/recovery and conflict rejection; actual EFI + 50-file runtime migration/no-op/rollback; Linux FAT/exFAT post-update filesystem checks; native Windows disposable-VHD update/no-op/rollback/remount check passed in CI at d4a2b93 | Physical host update and power-interruption testing, backup retention management |
| Build inputs | Rust 1.98.1 and Cargo.lock pinned; RC.13 release and its final-source CI completed successfully | Independent reproducible build attestation |
| Synthetic boot paths | Full QEMU matrix and two-image recovery passed; Alpine, Debian, Fedora, Ubuntu and Kali independent release-media boot checks passed on current commit `4dfa9c5` | Synthetic Btrfs fixtures are not ordinary Btrfs support; physical hardware still needs evidence |
| Debian 13.6 netinst x64 | Current-commit release builder → installer language selection in QEMU | Complete installation and physical machine verification |
| Fedora Workstation 44 x64 | Current-commit release builder → GNOME Display Manager in QEMU | Complete desktop session, installation and physical machine verification |
| Alpine 3.24.1 x64 | Current-commit release builder → Linux login prompt in QEMU | Physical machine and installation workflow |
| Ubuntu 26.04 Server x64 | Current-commit release builder → installer serial-mode selection in QEMU | Complete installation and physical machine verification |
| Kali 2026.2 netinst x64 | Current-commit release builder → language selection in QEMU | Complete installation and physical machine verification |
| Windows updater | Native disk selector and recoverable update flow passed on a disposable FAT32/exFAT VHD; VHD is detached and remounted after rollback | Physical USB update and power-interruption checks |
| Windows and WinPE boot | Code paths and synthetic tests exist | Official image installation/recovery checks |
| Other mainstream images | Families listed in ISO compatibility matrix | Exact version/hash/result rows, including rescue and appliance images |
| Failed image recovery | Broken ISO → menu → different Linux ISO passed in QEMU, including both attempts' resource cleanup; retry timeout disabled | Broad firmware/real-image regression coverage and cleanup-refusal behavior on hardware |
| Real hardware | Report tooling exists; public CSV has zero rows | Genuine device reports; no synthetic rows counted |
| Documentation | RC.13 user instructions, checksums, runtime notices and release notes are published | Physical-test troubleshooting records and final stable-release audit |

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




