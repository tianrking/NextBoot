//! LBA 映射模块
//!
//! 实现虚拟 LBA 到物理 LBA 的转换

use core::ops::Range;

/// LBA 映射条目
#[derive(Debug, Clone)]
pub struct LbaMapping {
    /// 虚拟 LBA 范围
    pub virtual_range: Range<u64>,
    /// 物理起始 LBA
    pub physical_start: u64,
}

impl LbaMapping {
    /// 创建新的映射
    pub fn new(virtual_start: u64, count: u64, physical_start: u64) -> Self {
        Self {
            virtual_range: virtual_start..(virtual_start + count),
            physical_start,
        }
    }

    /// 将虚拟 LBA 转换为物理 LBA
    pub fn translate(&self, virtual_lba: u64) -> Option<u64> {
        if self.virtual_range.contains(&virtual_lba) {
            Some(self.physical_start + (virtual_lba - self.virtual_range.start))
        } else {
            None
        }
    }
}

/// 多段映射表
///
/// 用于处理碎片化的 ISO 文件 (罕见，但需要支持)
pub struct MappingTable {
    mappings: alloc::vec::Vec<LbaMapping>,
    total_blocks: u64,
}

impl MappingTable {
    /// 创建单一连续映射 (最常见情况)
    pub fn contiguous(start_lba: u64, block_count: u64) -> Self {
        Self {
            mappings: alloc::vec![LbaMapping::new(0, block_count, start_lba)],
            total_blocks: block_count,
        }
    }

    /// 添加映射段
    pub fn add_segment(&mut self, virtual_start: u64, count: u64, physical_start: u64) {
        self.mappings.push(LbaMapping::new(virtual_start, count, physical_start));
        self.total_blocks = self.total_blocks.max(virtual_start + count);
    }

    /// 转换虚拟 LBA 到物理 LBA
    pub fn translate(&self, virtual_lba: u64) -> Option<u64> {
        for mapping in &self.mappings {
            if let Some(physical) = mapping.translate(virtual_lba) {
                return Some(physical);
            }
        }
        None
    }

    /// 获取总块数
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contiguous_mapping() {
        let table = MappingTable::contiguous(1000, 100);

        assert_eq!(table.translate(0), Some(1000));
        assert_eq!(table.translate(50), Some(1050));
        assert_eq!(table.translate(99), Some(1099));
        assert_eq!(table.translate(100), None);
    }

    #[test]
    fn test_multi_segment_mapping() {
        let mut table = MappingTable::contiguous(1000, 50);
        table.add_segment(50, 50, 2000);

        assert_eq!(table.translate(0), Some(1000));
        assert_eq!(table.translate(49), Some(1049));
        assert_eq!(table.translate(50), Some(2000));
        assert_eq!(table.translate(99), Some(2049));
    }
}
