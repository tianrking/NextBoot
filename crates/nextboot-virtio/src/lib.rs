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
use alloc::rc::Rc;
use core::cell::RefCell;
use bitflags::bitflags;

pub mod mapping;
pub mod protocol;

use mapping::MappingTable;

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

impl VirtualDeviceType {
    /// 获取设备类型描述
    pub fn description(&self) -> &'static str {
        match self {
            VirtualDeviceType::DvdRom => "Virtual DVD-ROM",
            VirtualDeviceType::HardDisk => "Virtual Hard Disk",
            VirtualDeviceType::UsbMassStorage => "Virtual USB Storage",
        }
    }

    /// 获取 UEFI 媒体类型
    pub fn uefi_media_type(&self) -> u8 {
        match self {
            VirtualDeviceType::DvdRom => 0x02, // CD-ROM
            VirtualDeviceType::HardDisk => 0x01, // Hard Disk
            VirtualDeviceType::UsbMassStorage => 0x01, // Hard Disk
        }
    }
}

/// 虚拟设备配置
#[derive(Debug, Clone)]
pub struct VirtualDeviceConfig {
    /// 设备类型
    pub device_type: VirtualDeviceType,
    /// ISO 文件起始 LBA
    pub iso_start_lba: u64,
    /// ISO 文件大小 (字节)
    pub iso_size: u64,
    /// 块大小
    pub block_size: u32,
    /// 设备名称
    pub device_name: alloc::string::String,
}

impl VirtualDeviceConfig {
    /// 创建新的虚拟设备配置
    pub fn new(
        device_type: VirtualDeviceType,
        iso_start_lba: u64,
        iso_size: u64,
        block_size: u32,
    ) -> Self {
        Self {
            device_type,
            iso_start_lba,
            iso_size,
            block_size,
            device_name: alloc::string::String::from("NextBoot Virtual Device"),
        }
    }

    /// 计算 ISO 文件占用的块数
    pub fn block_count(&self) -> u64 {
        (self.iso_size + self.block_size as u64 - 1) / self.block_size as u64
    }

    /// 设置设备名称
    pub fn with_name(mut self, name: &str) -> Self {
        self.device_name = alloc::string::String::from(name);
        self
    }
}

/// 物理读取函数类型
pub type PhysicalReadFn = fn(u64, &mut [u8]) -> Result<(), VirtIoError>;

/// 虚拟 Block IO 实例
pub struct VirtualBlockIo {
    /// 设备配置
    config: VirtualDeviceConfig,
    /// LBA 映射表
    mapping: MappingTable,
    /// 物理读取函数
    physical_read: Option<PhysicalReadFn>,
    /// 媒体 ID
    media_id: u32,
}

impl VirtualBlockIo {
    /// 创建新的虚拟 Block IO 实例
    pub fn new(config: VirtualDeviceConfig) -> Self {
        let block_count = config.block_count();
        let mapping = MappingTable::contiguous(config.iso_start_lba, block_count);

        Self {
            config,
            mapping,
            physical_read: None,
            media_id: 0x4E425453, // "NBTS" - NextBoot Storage
        }
    }

    /// 创建带有自定义映射的实例
    pub fn with_mapping(config: VirtualDeviceConfig, mapping: MappingTable) -> Self {
        Self {
            config,
            mapping,
            physical_read: None,
            media_id: 0x4E425453,
        }
    }

    /// 设置物理读取函数
    pub fn set_physical_read(&mut self, read_fn: PhysicalReadFn) {
        self.physical_read = Some(read_fn);
    }

    /// 获取块大小
    pub fn block_size(&self) -> u32 {
        self.config.block_size
    }

    /// 获取块数量
    pub fn block_count(&self) -> u64 {
        self.config.block_count()
    }

    /// 获取媒体 ID
    pub fn media_id(&self) -> u32 {
        self.media_id
    }

    /// 读取虚拟块
    ///
    /// # 参数
    /// - `media_id`: 媒体 ID (必须匹配)
    /// - `virtual_lba`: 虚拟 LBA (相对于 ISO 起始)
    /// - `buf`: 目标缓冲区
    pub fn read_blocks(&self, media_id: u32, virtual_lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        // 验证媒体 ID
        if media_id != self.media_id {
            return Err(VirtIoError::MediaChanged);
        }

        // 检查缓冲区对齐
        if buf.len() % self.config.block_size as usize != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        // 检查边界
        let blocks_to_read = buf.len() / self.config.block_size as usize;
        let max_lba = self.mapping.total_blocks();

        if virtual_lba >= max_lba {
            return Err(VirtIoError::OutOfBounds);
        }

        if virtual_lba + blocks_to_read as u64 > max_lba {
            return Err(VirtIoError::OutOfBounds);
        }

        // 执行读取
        let read_fn = self.physical_read.ok_or(VirtIoError::NoPhysicalRead)?;

        for i in 0..blocks_to_read {
            let current_lba = virtual_lba + i as u64;

            // 转换虚拟 LBA 到物理 LBA
            let physical_lba = self.mapping.translate(current_lba)
                .ok_or(VirtIoError::InvalidMapping)?;

            // 计算缓冲区偏移
            let offset = i * self.config.block_size as usize;
            let block_buf = &mut buf[offset..offset + self.config.block_size as usize];

            // 读取物理块
            read_fn(physical_lba, block_buf)?;
        }

        Ok(())
    }

    /// 写入虚拟块 (总是失败 - 只读)
    pub fn write_blocks(&self, _media_id: u32, _lba: u64, _buf: &[u8]) -> Result<(), VirtIoError> {
        // PRD 要求: 拦截所有 Write 请求
        Err(VirtIoError::WriteProtected)
    }

    /// 刷新缓冲区 (无操作)
    pub fn flush(&self) -> Result<(), VirtIoError> {
        Ok(())
    }

    /// 重置设备
    pub fn reset(&self, _extended_verification: bool) -> Result<(), VirtIoError> {
        Ok(())
    }

    /// 获取设备配置
    pub fn config(&self) -> &VirtualDeviceConfig {
        &self.config
    }

    /// 获取设备信息
    pub fn device_info(&self) -> VirtualDeviceInfo {
        VirtualDeviceInfo {
            device_type: self.config.device_type,
            block_size: self.config.block_size,
            block_count: self.config.block_count(),
            size_bytes: self.config.iso_size,
            read_only: true,
            media_present: true,
            media_id: self.media_id,
        }
    }
}

/// 虚拟设备信息
#[derive(Debug, Clone)]
pub struct VirtualDeviceInfo {
    /// 设备类型
    pub device_type: VirtualDeviceType,
    /// 块大小
    pub block_size: u32,
    /// 块数量
    pub block_count: u64,
    /// 总大小 (字节)
    pub size_bytes: u64,
    /// 是否只读
    pub read_only: bool,
    /// 媒体是否存在
    pub media_present: bool,
    /// 媒体 ID
    pub media_id: u32,
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
    /// 无效缓冲区大小
    InvalidBufferSize,
    /// 媒体已更改
    MediaChanged,
    /// 无效的 LBA 映射
    InvalidMapping,
    /// 未设置物理读取函数
    NoPhysicalRead,
    /// 设备错误
    DeviceError,
    /// CRC 错误
    CrcError,
}

impl core::fmt::Display for VirtIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VirtIoError::OutOfBounds => write!(f, "LBA out of bounds"),
            VirtIoError::WriteProtected => write!(f, "Device is write protected"),
            VirtIoError::ReadFailed => write!(f, "Read operation failed"),
            VirtIoError::InvalidArgument => write!(f, "Invalid argument"),
            VirtIoError::InvalidBufferSize => write!(f, "Buffer size must be multiple of block size"),
            VirtIoError::MediaChanged => write!(f, "Media changed"),
            VirtIoError::InvalidMapping => write!(f, "Invalid LBA mapping"),
            VirtIoError::NoPhysicalRead => write!(f, "No physical read function set"),
            VirtIoError::DeviceError => write!(f, "Device error"),
            VirtIoError::CrcError => write!(f, "CRC error"),
        }
    }
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
        /// 是否为逻辑分区
        const LOGICAL_PARTITION = 1 << 4;
        /// 是否启用写入缓存
        const WRITE_CACHING = 1 << 5;
    }
}

impl Default for MediaFlags {
    fn default() -> Self {
        MediaFlags::MEDIA_PRESENT | MediaFlags::READ_ONLY
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
    /// 媒体 ID
    pub media_id: u32,
}

impl VirtualMediaInfo {
    /// 从配置创建媒体信息
    pub fn from_config(config: &VirtualDeviceConfig) -> Self {
        let flags = MediaFlags::MEDIA_PRESENT
            | MediaFlags::READ_ONLY
            | MediaFlags::REMOVABLE
            | if config.block_size == 4096 {
                MediaFlags::USE_4K
            } else {
                MediaFlags::empty()
            };

        Self {
            block_size: config.block_size,
            last_block: config.block_count() - 1,
            flags,
            device_type_str: config.device_type.description(),
            media_id: 0x4E425453,
        }
    }

    /// 获取总大小 (字节)
    pub fn total_size(&self) -> u64 {
        (self.last_block + 1) * self.block_size as u64
    }
}

/// 虚拟设备管理器
pub struct VirtualDeviceManager {
    /// 已注册的虚拟设备
    devices: Vec<VirtualBlockIo>,
    /// 下一个设备索引
    next_index: usize,
}

impl VirtualDeviceManager {
    /// 创建新的设备管理器
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            next_index: 0,
        }
    }

    /// 注册新的虚拟设备
    pub fn register(&mut self, device: VirtualBlockIo) -> usize {
        let index = self.next_index;
        self.devices.push(device);
        self.next_index += 1;
        index
    }

    /// 获取设备
    pub fn get(&self, index: usize) -> Option<&VirtualBlockIo> {
        self.devices.get(index)
    }

    /// 获取可变引用
    pub fn get_mut(&mut self, index: usize) -> Option<&mut VirtualBlockIo> {
        self.devices.get_mut(index)
    }

    /// 获取设备数量
    pub fn count(&self) -> usize {
        self.devices.len()
    }

    /// 移除设备
    pub fn remove(&mut self, index: usize) -> Option<VirtualBlockIo> {
        if index < self.devices.len() {
            Some(self.devices.remove(index))
        } else {
            None
        }
    }
}

impl Default for VirtualDeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// ISO 文件映射信息
#[derive(Debug, Clone)]
pub struct IsoMapping {
    /// ISO 文件名
    pub filename: alloc::string::String,
    /// ISO 文件大小
    pub size: u64,
    /// 起始 LBA
    pub start_lba: u64,
    /// 块数量
    pub block_count: u64,
    /// 块大小
    pub block_size: u32,
    /// 设备类型
    pub device_type: VirtualDeviceType,
}

impl IsoMapping {
    /// 创建新的 ISO 映射
    pub fn new(
        filename: &str,
        size: u64,
        start_lba: u64,
        block_size: u32,
    ) -> Self {
        let block_count = (size + block_size as u64 - 1) / block_size as u64;

        // 根据 ISO 类型决定设备类型
        let device_type = if filename.to_lowercase().contains("windows") {
            VirtualDeviceType::DvdRom
        } else {
            VirtualDeviceType::HardDisk
        };

        Self {
            filename: alloc::string::String::from(filename),
            size,
            start_lba,
            block_count,
            block_size,
            device_type,
        }
    }

    /// 转换为虚拟设备配置
    pub fn to_config(&self) -> VirtualDeviceConfig {
        VirtualDeviceConfig::new(
            self.device_type,
            self.start_lba,
            self.size,
            self.block_size,
        ).with_name(&self.filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_device_config() {
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::DvdRom,
            1000,
            1024 * 1024 * 700, // 700 MB
            2048,
        );

        assert_eq!(config.block_count(), 358400);
        assert_eq!(config.device_type, VirtualDeviceType::DvdRom);
    }

    #[test]
    fn test_virtual_block_io() {
        let config = VirtualDeviceConfig::new(
            VirtualDeviceType::HardDisk,
            1000,
            1024 * 1024, // 1 MB
            512,
        );

        let mut vbio = VirtualBlockIo::new(config);

        // 设置物理读取函数
        vbio.set_physical_read(|lba, buf| {
            // 模拟读取
            for b in buf.iter_mut() {
                *b = 0xAA;
            }
            Ok(())
        });

        // 测试读取
        let mut buf = [0u8; 512];
        let result = vbio.read_blocks(vbio.media_id(), 0, &mut buf);
        assert!(result.is_ok());

        // 测试写入 (应该失败)
        let result = vbio.write_blocks(vbio.media_id(), 0, &[0u8; 512]);
        assert!(result.is_err());
    }
}
