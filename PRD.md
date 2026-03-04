项目名称： NextBoot (基于 Rust 的 UEFI 启动加载器)
核心逻辑： 无需格式化 -> 拖入 ISO -> 模拟虚拟光驱 -> 欺骗操作系统启动。

1. 核心架构与存储规范 (解决 "新老 U 盘 bug" 的关键)
这部分是地基，必须死板地遵守标准，避免 Ventoy 的 Magic Partition 带来的兼容性问题。

分区表标准 (P0)：

必须使用标准 GPT (GUID Partition Table) 分区表。

严禁使用 MBR/GPT 混合分区 (这是导致很多主板和 4K 盘不识别的根源)。

分区布局 (P0)：

Partition 1 (ESP): 仅存放 Bootloader (.efi) 和配置文件。文件系统 FAT32，大小固定 (如 200MB)。

Partition 2 (Data): 存放 ISO/IMG 镜像。文件系统 exFAT 或 NTFS (支持 >4GB 文件)。占用剩余所有空间。

扇区对齐 (P0)：

代码层必须动态读取物理设备的 Block Size (512B 或 4K)，严禁硬编码 512 字节偏移量。

所有读写操作必须以物理扇区为最小单位对齐。

2. 功能模块详述
模块 A: 启动与文件发现 (UEFI Stage)
文件遍历 (P0)：

启动后自动挂载 Partition 2 (Data 区)。

递归扫描指定目录 (如 /ISO) 下的所有 .iso, .wim, .img, .vhd 文件。

过滤逻辑： 忽略隐藏文件、系统文件。

菜单渲染 (P1)：

提供基于文本的 GUI (利用 UEFI GOP 协议)。

显示：文件名、文件大小、检测到的文件系统类型。

交互：上下键选择，回车确认。

缓存机制 (P2)：

首次启动扫描全盘建立索引文件 (filelist.json) 存入 ESP 分区，加快二次启动速度。

模块 B: 虚拟化层 (The Hook - 核心难点)
这是实现“免格启动”的核心。

Block IO 劫持 (P0)：

实现一个 Virtual Block IO Protocol 驱动。

映射逻辑： 当上层请求读取虚拟设备 LBA x 时 -> 计算 ISO 文件在物理 U 盘上的偏移量 y -> 转发请求读取物理 LBA y。

只读保护： 拦截所有 Write 请求，返回 Access Denied 或直接丢弃，防止 ISO 被篡改。

CD-ROM 模拟 (P0)：

向 UEFI 注册设备路径 (Device Path) 时，必须声明设备类型为 CD-ROM 或 HardDisk (根据 ISO 类型动态切换)。

对于 Windows ISO：模拟为 DVD 光驱。

对于 Linux LiveCD：模拟为 USB 硬盘或光驱均可。

模块 C: 操作系统引导链 (OS Handoff)
针对不同 OS 的特殊处理。

Linux 引导 (P0)：

解析 ISO 内部的 grub.cfg 或 isolinux.cfg。

关键动作： 提取 Kernel (vmlinuz) 和 Initrd (initrd.img) 加载到内存。

参数注入： 自动在 Cmdline 中注入 findiso=/ISO/ubuntu.iso 或 iso-scan/filename= 参数 (不同发行版不同)。

Windows 引导 (P1)：

内存补丁 (IVT)： Windows 启动后会重置 USB 驱动，导致虚拟盘丢失。

方案： 需要编写一个微型驱动 (类似 WinPE 的 RAMDisk 驱动)，在 bootmgfw.efi 加载前注入内存 ACPI 表，确保 Windows 内核接管后仍能找到那个 ISO 文件。

WIM/VHD 启动 (P2)：

直接支持从 .wim (Windows Image) 启动 PE 环境。

支持从 .vhd (虚拟硬盘) 差分启动原生 Windows。

模块 D: 配置与持久化 (User Experience)
状态记忆 (P1)：

记录上次选择的 ISO，下次启动默认选中。

主题支持 (P2)：

支持加载背景图 (BMP/PNG) 和自定义字体。

安全启动 (Secure Boot) (P3)：

这也是个大坑。前期建议关闭 Secure Boot 开发。后期需申请微软签名 shim 或提供自定义 Key 导入工具。

3. 技术栈与开发约束
语言： Rust (利用 uefi-rs, alloc 库)。

构建目标： x86_64-unknown-uefi。

内存管理：

严禁使用未初始化的指针。

所有内存申请必须检查 Out of Resources 状态。

错误处理：

遇到 4K/512B 扇区不匹配时，必须在屏幕弹窗报错，而不是静默崩溃。

4. 交付阶段规划 (Roadmap)
MVP (最小可行性版本)：

能读 U 盘 exFAT 分区。

能列出 ISO。

能成功启动 Ubuntu (因为 Linux 只需要改 Cmdline，不需要复杂的内存 Hook)。

Beta (Windows 支持)：

攻克 Windows 的 Block IO 劫持难点。

支持 Windows 安装镜像启动。

Release (兼容性修复)：

测试各品牌主板 (Dell, Lenovo, HP) 的 UEFI 实现差异。

适配 4K Native 的高性能 U 盘。