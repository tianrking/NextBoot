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

use mapping::{ByteMappingTable, MappingTable};

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
            VirtualDeviceType::DvdRom => 0x02,         // CD-ROM
            VirtualDeviceType::HardDisk => 0x01,       // Hard Disk
            VirtualDeviceType::UsbMassStorage => 0x01, // Hard Disk
        }
    }
}

/// El Torito CD-ROM boot image location used in UEFI device paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdRomBootInfo {
    pub boot_entry: u32,
    pub image_lba: u64,
    pub image_block_count: u64,
}

impl CdRomBootInfo {
    pub fn new(boot_entry: u32, image_lba: u64, image_block_count: u64) -> Self {
        Self {
            boot_entry,
            image_lba,
            image_block_count: image_block_count.max(1),
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
    /// 底层物理块大小
    pub physical_block_size: u32,
    /// 设备名称
    pub device_name: alloc::string::String,
    /// Optional El Torito boot image used by virtual CD-ROM device paths.
    pub cdrom_boot: Option<CdRomBootInfo>,
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
            physical_block_size: block_size,
            device_name: alloc::string::String::from("NextBoot Virtual Device"),
            cdrom_boot: None,
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

    /// 设置底层物理块大小。
    pub fn with_physical_block_size(mut self, physical_block_size: u32) -> Self {
        self.physical_block_size = physical_block_size;
        self
    }

    /// Set the El Torito boot image for virtual CD-ROM media.
    pub fn with_cdrom_boot(mut self, boot: CdRomBootInfo) -> Self {
        self.cdrom_boot = Some(boot);
        self
    }
}

/// 物理读取函数类型
pub type PhysicalReadFn = fn(u64, &mut [u8]) -> Result<(), VirtIoError>;

/// 可携带状态的物理块读取器。
pub trait PhysicalReader {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError>;
}

struct FnPhysicalReader {
    read_fn: PhysicalReadFn,
}

impl PhysicalReader for FnPhysicalReader {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        (self.read_fn)(lba, buf)
    }
}

/// 虚拟 Block IO 实例
pub struct VirtualBlockIo {
    /// 设备配置
    config: VirtualDeviceConfig,
    /// 字节级映射表
    byte_mapping: ByteMappingTable,
    /// 物理读取函数
    physical_read: Option<Box<dyn PhysicalReader>>,
    /// 媒体 ID
    media_id: u32,
}

impl VirtualBlockIo {
    /// 创建新的虚拟 Block IO 实例
    pub fn new(config: VirtualDeviceConfig) -> Self {
        let block_count = config.block_count();
        let mapping = MappingTable::contiguous(config.iso_start_lba, block_count);
        let byte_mapping = ByteMappingTable::from_block_mapping(
            &mapping,
            config.block_size as u64,
            config.physical_block_size as u64,
        );

        Self {
            config,
            byte_mapping,
            physical_read: None,
            media_id: 0x4E425453, // "NBTS" - NextBoot Storage
        }
    }

    /// 创建带有自定义映射的实例
    pub fn with_mapping(config: VirtualDeviceConfig, mapping: MappingTable) -> Self {
        let byte_mapping = ByteMappingTable::from_block_mapping(
            &mapping,
            config.block_size as u64,
            config.physical_block_size as u64,
        );

        Self {
            config,
            byte_mapping,
            physical_read: None,
            media_id: 0x4E425453,
        }
    }

    /// 创建带有字节级映射的实例。
    pub fn with_byte_mapping(config: VirtualDeviceConfig, byte_mapping: ByteMappingTable) -> Self {
        Self {
            config,
            byte_mapping,
            physical_read: None,
            media_id: 0x4E425453,
        }
    }

    /// 从文件系统 extent 创建虚拟 Block IO。
    pub fn from_file_extents(config: VirtualDeviceConfig, extents: &[(u64, u64, u64)]) -> Self {
        let mut byte_mapping = ByteMappingTable::from_file_extents(
            extents,
            config.iso_size,
            config.physical_block_size as u64,
        );
        byte_mapping.optimize();
        Self::with_byte_mapping(config, byte_mapping)
    }

    /// 设置物理读取函数
    pub fn set_physical_read(&mut self, read_fn: PhysicalReadFn) {
        self.physical_read = Some(Box::new(FnPhysicalReader { read_fn }));
    }

    /// 设置可携带状态的物理读取器。
    pub fn set_physical_reader<R>(&mut self, reader: R)
    where
        R: PhysicalReader + 'static,
    {
        self.physical_read = Some(Box::new(reader));
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
    pub fn read_blocks(
        &self,
        media_id: u32,
        virtual_lba: u64,
        buf: &mut [u8],
    ) -> Result<(), VirtIoError> {
        // 验证媒体 ID
        if media_id != self.media_id {
            return Err(VirtIoError::MediaChanged);
        }

        if self.config.block_size == 0 {
            return Err(VirtIoError::InvalidArgument);
        }

        // 检查缓冲区对齐
        if buf.len() % self.config.block_size as usize != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        // 检查边界
        let blocks_to_read = buf.len() / self.config.block_size as usize;
        let max_lba = self.config.block_count();

        if virtual_lba >= max_lba {
            return Err(VirtIoError::OutOfBounds);
        }

        if virtual_lba
            .checked_add(blocks_to_read as u64)
            .map_or(true, |end| end > max_lba)
        {
            return Err(VirtIoError::OutOfBounds);
        }

        let reader = self
            .physical_read
            .as_ref()
            .ok_or(VirtIoError::NoPhysicalRead)?;
        let virtual_offset = virtual_lba
            .checked_mul(self.config.block_size as u64)
            .ok_or(VirtIoError::OutOfBounds)?;
        self.read_virtual_bytes(reader.as_ref(), virtual_offset, buf)
    }

    /// 读取虚拟介质上的任意字节范围。
    ///
    /// Disk IO 协议和部分文件系统驱动会发起非块对齐读取，因此这里直接
    /// 复用字节级映射表，而不是要求调用方按 Block IO 粒度对齐。
    pub fn read_bytes(
        &self,
        media_id: u32,
        virtual_offset: u64,
        buf: &mut [u8],
    ) -> Result<(), VirtIoError> {
        if media_id != self.media_id {
            return Err(VirtIoError::MediaChanged);
        }

        if buf.is_empty() {
            return Ok(());
        }

        let end = virtual_offset
            .checked_add(buf.len() as u64)
            .ok_or(VirtIoError::OutOfBounds)?;
        if end > self.config.iso_size {
            return Err(VirtIoError::OutOfBounds);
        }

        let reader = self
            .physical_read
            .as_ref()
            .ok_or(VirtIoError::NoPhysicalRead)?;
        self.read_virtual_bytes(reader.as_ref(), virtual_offset, buf)
    }

    fn read_virtual_bytes(
        &self,
        reader: &dyn PhysicalReader,
        virtual_offset: u64,
        buf: &mut [u8],
    ) -> Result<(), VirtIoError> {
        buf.fill(0);

        if virtual_offset >= self.config.iso_size {
            return Ok(());
        }

        let readable = (self.config.iso_size - virtual_offset).min(buf.len() as u64);
        let ranges = self
            .byte_mapping
            .translate_range_sparse(virtual_offset, readable)
            .ok_or(VirtIoError::InvalidMapping)?;

        let physical_block_size = self.config.physical_block_size as usize;
        if physical_block_size == 0 {
            return Err(VirtIoError::InvalidArgument);
        }

        let mut scratch = alloc::vec![0u8; physical_block_size];

        for (dst_offset, physical_byte_start, byte_count) in ranges {
            let dst_offset =
                usize::try_from(dst_offset).map_err(|_| VirtIoError::InvalidArgument)?;
            let mut copied = 0usize;
            let mut remaining =
                usize::try_from(byte_count).map_err(|_| VirtIoError::InvalidArgument)?;
            let mut physical_byte = physical_byte_start;

            while remaining > 0 {
                let physical_lba = physical_byte / physical_block_size as u64;
                let in_block_offset = (physical_byte % physical_block_size as u64) as usize;
                let copy_size = (physical_block_size - in_block_offset).min(remaining);

                reader.read_blocks(physical_lba, &mut scratch)?;

                let write_start = dst_offset + copied;
                buf[write_start..write_start + copy_size]
                    .copy_from_slice(&scratch[in_block_offset..in_block_offset + copy_size]);

                copied += copy_size;
                physical_byte += copy_size as u64;
                remaining -= copy_size;
            }
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
            VirtIoError::InvalidBufferSize => {
                write!(f, "Buffer size must be multiple of block size")
            }
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
    pub fn new(filename: &str, size: u64, start_lba: u64, block_size: u32) -> Self {
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
        VirtualDeviceConfig::new(self.device_type, self.start_lba, self.size, self.block_size)
            .with_name(&self.filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_lba_read(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        for b in buf.iter_mut() {
            *b = lba as u8;
        }
        Ok(())
    }

    fn patterned_4k_read(lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        for (index, b) in buf.iter_mut().enumerate() {
            *b = (lba as u8)
                .wrapping_mul(16)
                .wrapping_add((index / 1024) as u8);
        }
        Ok(())
    }

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
    fn virtual_device_config_keeps_cdrom_boot_info() {
        let boot = CdRomBootInfo::new(2, 48, 0);
        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 4096, 2048)
            .with_cdrom_boot(boot);

        assert_eq!(config.cdrom_boot, Some(CdRomBootInfo::new(2, 48, 1)));
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
        vbio.set_physical_read(|_lba, buf| {
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

    #[test]
    fn test_virtual_block_io_reads_fragmented_file_extents() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 1024, 512);
        let extents = [(0, 10, 1), (1, 20, 1)];
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
        vbio.set_physical_read(fill_lba_read);

        let mut buf = [0u8; 1024];
        vbio.read_blocks(vbio.media_id(), 0, &mut buf)
            .expect("fragmented extent read");

        assert!(buf[..512].iter().all(|b| *b == 10));
        assert!(buf[512..].iter().all(|b| *b == 20));
    }

    #[test]
    fn test_virtual_block_io_maps_2048_virtual_blocks_to_4k_physical_blocks() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 8192, 2048)
            .with_physical_block_size(4096);
        let extents = [(0, 2, 2)];
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
        vbio.set_physical_read(patterned_4k_read);

        let mut buf = [0u8; 8192];
        vbio.read_blocks(vbio.media_id(), 0, &mut buf)
            .expect("4K-backed DVD read");

        assert_eq!(buf[0], 32);
        assert_eq!(buf[1023], 32);
        assert_eq!(buf[1024], 33);
        assert_eq!(buf[2048], 34);
        assert_eq!(buf[3072], 35);
        assert_eq!(buf[4096], 48);
        assert_eq!(buf[5120], 49);
        assert_eq!(buf[7168], 51);
    }

    #[test]
    fn test_virtual_block_io_zero_fills_file_tail_padding() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 600, 512);
        let extents = [(0, 7, 2)];
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
        vbio.set_physical_read(fill_lba_read);

        let mut buf = [0xFFu8; 1024];
        vbio.read_blocks(vbio.media_id(), 0, &mut buf)
            .expect("tail padding read");

        assert!(buf[..512].iter().all(|b| *b == 7));
        assert!(buf[512..600].iter().all(|b| *b == 8));
        assert!(buf[600..].iter().all(|b| *b == 0));
    }

    #[test]
    fn test_virtual_block_io_zero_fills_sparse_byte_mapping() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 1536, 512);
        let mut byte_mapping = ByteMappingTable::empty();
        byte_mapping.add_segment(0, 512, 10 * 512);
        byte_mapping.add_segment(1024, 512, 20 * 512);
        byte_mapping.truncate(1536);
        let mut vbio = VirtualBlockIo::with_byte_mapping(config, byte_mapping);
        vbio.set_physical_read(fill_lba_read);

        let mut buf = [0xFFu8; 1536];
        vbio.read_blocks(vbio.media_id(), 0, &mut buf)
            .expect("sparse byte mapping read");

        assert!(buf[..512].iter().all(|b| *b == 10));
        assert!(buf[512..1024].iter().all(|b| *b == 0));
        assert!(buf[1024..].iter().all(|b| *b == 20));
    }

    #[test]
    fn test_virtual_block_io_reads_unaligned_bytes() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::DvdRom, 0, 4096, 2048)
            .with_physical_block_size(4096);
        let extents = [(0, 2, 1)];
        let mut vbio = VirtualBlockIo::from_file_extents(config, &extents);
        vbio.set_physical_read(patterned_4k_read);

        let mut buf = [0u8; 3];
        vbio.read_bytes(vbio.media_id(), 1023, &mut buf)
            .expect("unaligned byte read");

        assert_eq!(buf, [32, 33, 33]);
    }

    #[test]
    fn test_virtual_block_io_rejects_out_of_range_byte_read() {
        let config = VirtualDeviceConfig::new(VirtualDeviceType::HardDisk, 0, 600, 512);
        let mut vbio = VirtualBlockIo::new(config);
        vbio.set_physical_read(fill_lba_read);

        let mut buf = [0u8; 2];
        assert!(matches!(
            vbio.read_bytes(vbio.media_id(), 599, &mut buf),
            Err(VirtIoError::OutOfBounds)
        ));
    }
}
