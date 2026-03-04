//! 虚拟 Block IO 驱动
//!
//! 这是 NextBoot 的核心模块，实现 ISO 文件到虚拟设备的映射。
//!
//! # 工作原理
//! 1. 将 ISO 文件在物理设备上的位置映射为虚拟 LBA
//! 2. 拦截读取请求，转换为物理设备读取
//! 3. 拦截写入请求，返回只读错误
//!
//! # PRD 对应
//! - 模块 B: 虚拟化层 (P0)

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use bitflags::bitflags;

pub mod mapping;
pub mod protocol;

/// 虚拟设备类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualDeviceType {
    /// 模拟 DVD 光驱
    DvdRom,
    /// 模拟硬盘
    HardDisk,
    /// 模拟 USB 存储设备
    UsbMassStorage,
}

/// 虚拟设备配置
#[derive(Debug, Clone)]
pub struct VirtualDeviceConfig {
    /// 设备类型
    pub device_type: VirtualDeviceType,
    /// 物理设备 Block IO 句柄
    pub physical_handle: u64,
    /// ISO 文件起始 LBA
    pub iso_start_lba: u64,
    /// ISO 文件大小 (字节)
    pub iso_size: u64,
    /// 块大小
    pub block_size: u32,
}

impl VirtualDeviceConfig {
    /// 计算 ISO 文件占用的块数
    pub fn block_count(&self) -> u64 {
        (self.iso_size + self.block_size as u64 - 1) / self.block_size as u64
    }
}

/// 虚拟 Block IO 实例
pub struct VirtualBlockIo {
    config: VirtualDeviceConfig,
    /// 物理读取函数指针
    physical_read: fn(u64, &mut [u8]) -> Result<(), VirtIoError>,
}

impl VirtualBlockIo {
    /// 创建新的虚拟 Block IO 实例
    pub fn new(config: VirtualDeviceConfig) -> Self {
        Self {
            config,
            physical_read: stub_read,
        }
    }

    /// 设置物理读取函数
    pub fn set_physical_read(&mut self, read_fn: fn(u64, &mut [u8]) -> Result<(), VirtIoError>) {
        self.physical_read = read_fn;
    }

    /// 读取虚拟块
    ///
    /// # 参数
    /// - `virtual_lba`: 虚拟 LBA (相对于 ISO 起始)
    /// - `buf`: 目标缓冲区
    pub fn read_blocks(&self, virtual_lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        // 检查边界
        let max_lba = self.config.block_count();
        if virtual_lba >= max_lba {
            return Err(VirtIoError::OutOfBounds);
        }

        // 计算物理 LBA
        let physical_lba = self.config.iso_start_lba + virtual_lba;

        // 执行物理读取
        (self.physical_read)(physical_lba, buf)
    }

    /// 写入虚拟块 (总是失败 - 只读)
    pub fn write_blocks(&self, _lba: u64, _buf: &[u8]) -> Result<(), VirtIoError> {
        // PRD 要求: 拦截所有 Write 请求
        Err(VirtIoError::WriteProtected)
    }

    /// 获取设备信息
    pub fn device_info(&self) -> &VirtualDeviceConfig {
        &self.config
    }
}

/// 虚拟 IO 错误
#[derive(Debug, Clone, Copy)]
pub enum VirtIoError {
    /// 超出边界
    OutOfBounds,
    /// 写保护
    WriteProtected,
    /// 读取失败
    ReadFailed,
    /// 无效参数
    InvalidArgument,
}

/// 占位读取函数
fn stub_read(_lba: u64, _buf: &mut [u8]) -> Result<(), VirtIoError> {
    Err(VirtIoError::ReadFailed)
}

bitflags! {
    /// UEFI Block IO 媒体标志
    #[derive(Debug, Clone, Copy)]
    pub struct MediaFlags: u32 {
        /// 媒体是否存在
        const MEDIA_PRESENT = 1 << 0;
        /// 是否为只读
        const READ_ONLY = 1 << 1;
        /// 是否为可移动设备
        const REMOVABLE = 1 << 2;
        /// 是否使用 4K 扇区
        const USE_4K = 1 << 3;
    }
}

/// 虚拟设备媒体信息
#[derive(Debug, Clone)]
pub struct VirtualMediaInfo {
    /// 块大小
    pub block_size: u32,
    /// 最后一个块 LBA
    pub last_block: u64,
    /// 媒体标志
    pub flags: MediaFlags,
    /// 设备类型字符串
    pub device_type_str: &'static str,
}

impl VirtualMediaInfo {
    /// 从配置创建媒体信息
    pub fn from_config(config: &VirtualDeviceConfig) -> Self {
        let flags = MediaFlags::MEDIA_PRESENT
            | MediaFlags::READ_ONLY
            | if config.block_size == 4096 {
                MediaFlags::USE_4K
            } else {
                MediaFlags::empty()
            };

        Self {
            block_size: config.block_size,
            last_block: config.block_count() - 1,
            flags,
            device_type_str: match config.device_type {
                VirtualDeviceType::DvdRom => "DVD-ROM",
                VirtualDeviceType::HardDisk => "HDD",
                VirtualDeviceType::UsbMassStorage => "USB",
            },
        }
    }
}
