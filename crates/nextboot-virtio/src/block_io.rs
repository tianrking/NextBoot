use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::mapping::{ByteMappingTable, MappingTable};
use crate::{VirtIoError, VirtualDeviceConfig, VirtualDeviceInfo};

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

/// In-memory bytes exposed at a virtual media offset.
#[derive(Debug, Clone)]
pub struct MemoryOverlay {
    pub virtual_offset: u64,
    pub data: Vec<u8>,
}

impl MemoryOverlay {
    pub fn new(virtual_offset: u64, data: Vec<u8>) -> Self {
        Self {
            virtual_offset,
            data,
        }
    }

    fn end(&self) -> Option<u64> {
        self.virtual_offset.checked_add(self.data.len() as u64)
    }
}

/// 虚拟 Block IO 实例
pub struct VirtualBlockIo {
    /// 设备配置
    config: VirtualDeviceConfig,
    /// 字节级映射表
    byte_mapping: ByteMappingTable,
    /// In-memory overrides for bytes that do not exist on the source disk.
    memory_overlays: Vec<MemoryOverlay>,
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
            memory_overlays: Vec::new(),
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
            memory_overlays: Vec::new(),
            physical_read: None,
            media_id: 0x4E425453,
        }
    }

    /// 创建带有字节级映射的实例。
    pub fn with_byte_mapping(config: VirtualDeviceConfig, byte_mapping: ByteMappingTable) -> Self {
        Self {
            config,
            byte_mapping,
            memory_overlays: Vec::new(),
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

    /// Add an owned in-memory overlay at a virtual byte offset.
    pub fn add_memory_overlay(&mut self, overlay: MemoryOverlay) -> Result<(), VirtIoError> {
        let end = overlay.end().ok_or(VirtIoError::InvalidArgument)?;
        if end > self.config.iso_size {
            self.config.iso_size = end;
            self.byte_mapping.set_total_bytes(end);
        }

        self.memory_overlays.push(overlay);
        Ok(())
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
            .is_none_or(|end| end > max_lba)
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

        self.apply_memory_overlays(virtual_offset, readable, buf)?;

        Ok(())
    }

    fn apply_memory_overlays(
        &self,
        virtual_offset: u64,
        readable: u64,
        buf: &mut [u8],
    ) -> Result<(), VirtIoError> {
        let read_end = virtual_offset
            .checked_add(readable)
            .ok_or(VirtIoError::OutOfBounds)?;
        for overlay in &self.memory_overlays {
            let Some(overlay_end) = overlay.end() else {
                return Err(VirtIoError::InvalidArgument);
            };
            let overlap_start = virtual_offset.max(overlay.virtual_offset);
            let overlap_end = read_end.min(overlay_end);
            if overlap_start >= overlap_end {
                continue;
            }

            let dst_start = usize::try_from(overlap_start - virtual_offset)
                .map_err(|_| VirtIoError::InvalidArgument)?;
            let src_start = usize::try_from(overlap_start - overlay.virtual_offset)
                .map_err(|_| VirtIoError::InvalidArgument)?;
            let len = usize::try_from(overlap_end - overlap_start)
                .map_err(|_| VirtIoError::InvalidArgument)?;
            buf[dst_start..dst_start + len]
                .copy_from_slice(&overlay.data[src_start..src_start + len]);
        }

        Ok(())
    }

    /// 写入虚拟块 (总是失败 - 只读)
    pub fn write_blocks(&self, _media_id: u32, _lba: u64, _buf: &[u8]) -> Result<(), VirtIoError> {
        // 需求要求: 拦截所有 Write 请求
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
