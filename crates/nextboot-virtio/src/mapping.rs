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

    /// 获取映射的块数
    pub fn block_count(&self) -> u64 {
        self.virtual_range.end - self.virtual_range.start
    }
}

/// 多段映射表
///
/// 用于处理碎片化的 ISO 文件 (罕见，但需要支持)
#[derive(Debug, Clone)]
pub struct MappingTable {
    mappings: alloc::vec::Vec<LbaMapping>,
    total_blocks: u64,
}

impl MappingTable {
    /// 创建空映射表
    pub fn empty() -> Self {
        Self {
            mappings: alloc::vec::Vec::new(),
            total_blocks: 0,
        }
    }

    /// 创建单一连续映射 (最常见情况)
    pub fn contiguous(start_lba: u64, block_count: u64) -> Self {
        Self {
            mappings: alloc::vec![LbaMapping::new(0, block_count, start_lba)],
            total_blocks: block_count,
        }
    }

    /// 创建具有多个段的映射表
    pub fn with_segments(segments: &[(u64, u64, u64)]) -> Self {
        let mut mappings = alloc::vec::Vec::new();
        let mut total_blocks = 0u64;

        for (virtual_start, count, physical_start) in segments {
            mappings.push(LbaMapping::new(*virtual_start, *count, *physical_start));
            total_blocks = total_blocks.max(virtual_start + count);
        }

        Self {
            mappings,
            total_blocks,
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

    /// 批量转换
    pub fn translate_range(&self, virtual_start: u64, count: u64) -> Option<alloc::vec::Vec<(u64, u64)>> {
        let mut result = alloc::vec::Vec::new();
        let mut remaining = count;
        let mut current_virtual = virtual_start;

        while remaining > 0 {
            if let Some(physical) = self.translate(current_virtual) {
                // 查找连续的块
                let mut contiguous_count = 1u64;
                while contiguous_count < remaining {
                    let next_virtual = current_virtual + contiguous_count;
                    let next_physical = physical + contiguous_count;

                    if self.translate(next_virtual) != Some(next_physical) {
                        break;
                    }
                    contiguous_count += 1;
                }

                result.push((physical, contiguous_count));
                current_virtual += contiguous_count;
                remaining -= contiguous_count;
            } else {
                return None;
            }
        }

        Some(result)
    }

    /// 获取总块数
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// 获取映射段数
    pub fn segment_count(&self) -> usize {
        self.mappings.len()
    }

    /// 检查是否为连续映射
    pub fn is_contiguous(&self) -> bool {
        self.mappings.len() == 1
    }

    /// 获取所有映射
    pub fn mappings(&self) -> &[LbaMapping] {
        &self.mappings
    }

    /// 优化映射表 (合并相邻段)
    pub fn optimize(&mut self) {
        if self.mappings.len() <= 1 {
            return;
        }

        // 按虚拟 LBA 排序
        self.mappings.sort_by_key(|m| m.virtual_range.start);

        let mut optimized = alloc::vec::Vec::new();
        let mut current = self.mappings[0].clone();

        for mapping in self.mappings.iter().skip(1) {
            // 检查是否可以合并
            if current.virtual_range.end == mapping.virtual_range.start
                && current.physical_start + current.block_count() == mapping.physical_start
            {
                // 合并
                current.virtual_range.end = mapping.virtual_range.end;
            } else {
                optimized.push(current);
                current = mapping.clone();
            }
        }
        optimized.push(current);

        self.mappings = optimized;
    }
}

impl Default for MappingTable {
    fn default() -> Self {
        Self::empty()
    }
}

/// 碎片化信息
#[derive(Debug, Clone)]
pub struct FragmentationInfo {
    /// 总段数
    pub segment_count: usize,
    /// 是否碎片化
    pub is_fragmented: bool,
    /// 碎片化比例 (0.0 - 1.0)
    pub fragmentation_ratio: f32,
}

impl MappingTable {
    /// 获取碎片化信息
    pub fn fragmentation_info(&self) -> FragmentationInfo {
        let segment_count = self.mappings.len();
        let is_fragmented = segment_count > 1;

        // 计算理论最小段数 (基于大小)
        let fragmentation_ratio = if segment_count <= 1 {
            0.0
        } else {
            (segment_count - 1) as f32 / self.total_blocks.max(1) as f32
        };

        FragmentationInfo {
            segment_count,
            is_fragmented,
            fragmentation_ratio,
        }
    }
}

/// 地址范围
#[derive(Debug, Clone, Copy)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
}

impl AddressRange {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end
    }

    pub fn size(&self) -> u64 {
        self.end - self.start
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
        assert!(table.is_contiguous());
    }

    #[test]
    fn test_multi_segment_mapping() {
        let mut table = MappingTable::contiguous(1000, 50);
        table.add_segment(50, 50, 2000);

        assert_eq!(table.translate(0), Some(1000));
        assert_eq!(table.translate(49), Some(1049));
        assert_eq!(table.translate(50), Some(2000));
        assert_eq!(table.translate(99), Some(2049));
        assert!(!table.is_contiguous());
    }

    #[test]
    fn test_translate_range() {
        let table = MappingTable::contiguous(1000, 100);

        let result = table.translate_range(0, 10);
        assert!(result.is_some());
        let ranges = result.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], (1000, 10));
    }

    #[test]
    fn test_optimize() {
        let mut table = MappingTable::with_segments(&[
            (0, 50, 1000),
            (50, 50, 1050), // 连续，可以合并
        ]);

        assert_eq!(table.segment_count(), 2);
        table.optimize();
        assert_eq!(table.segment_count(), 1);
    }

    #[test]
    fn test_fragmentation_info() {
        let contiguous = MappingTable::contiguous(0, 100);
        let info = contiguous.fragmentation_info();
        assert!(!info.is_fragmented);
        assert_eq!(info.segment_count, 1);

        let mut fragmented = MappingTable::contiguous(0, 50);
        fragmented.add_segment(50, 50, 2000);
        let info = fragmented.fragmentation_info();
        assert!(info.is_fragmented);
        assert_eq!(info.segment_count, 2);
    }
}
