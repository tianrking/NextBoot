use super::MappingTable;
use alloc::vec::Vec;
use core::ops::Range;

/// 字节级映射条目。
///
/// 这用于虚拟介质块大小与物理存储块大小不同的情况，例如在 4096B
/// 物理扇区的 SSD/NVMe 上暴露 2048B DVD-ROM 块。
#[derive(Debug, Clone)]
pub struct ByteMapping {
    /// 虚拟字节范围
    pub virtual_range: Range<u64>,
    /// 物理字节起点
    pub physical_start: u64,
}

impl ByteMapping {
    pub fn new(virtual_start: u64, byte_count: u64, physical_start: u64) -> Self {
        Self {
            virtual_range: virtual_start..(virtual_start + byte_count),
            physical_start,
        }
    }

    pub fn translate(&self, virtual_offset: u64) -> Option<u64> {
        if self.virtual_range.contains(&virtual_offset) {
            Some(self.physical_start + (virtual_offset - self.virtual_range.start))
        } else {
            None
        }
    }

    pub fn byte_count(&self) -> u64 {
        self.virtual_range.end - self.virtual_range.start
    }
}

/// 字节级多段映射表。
#[derive(Debug, Clone)]
pub struct ByteMappingTable {
    mappings: Vec<ByteMapping>,
    total_bytes: u64,
}

impl ByteMappingTable {
    pub fn empty() -> Self {
        Self {
            mappings: Vec::new(),
            total_bytes: 0,
        }
    }

    pub fn contiguous(physical_start: u64, byte_count: u64) -> Self {
        Self {
            mappings: alloc::vec![ByteMapping::new(0, byte_count, physical_start)],
            total_bytes: byte_count,
        }
    }

    pub fn from_block_mapping(
        mapping: &MappingTable,
        virtual_block_size: u64,
        physical_block_size: u64,
    ) -> Self {
        let mut table = Self::empty();

        for segment in mapping.mappings() {
            table.add_segment(
                segment.virtual_range.start * virtual_block_size,
                segment.block_count() * virtual_block_size,
                segment.physical_start * physical_block_size,
            );
        }

        table
    }

    pub fn from_file_extents(
        extents: &[(u64, u64, u64)],
        file_size: u64,
        physical_block_size: u64,
    ) -> Self {
        let mut table = Self::empty();

        for (virtual_block_start, physical_lba, block_count) in extents {
            table.add_segment(
                virtual_block_start * physical_block_size,
                block_count * physical_block_size,
                physical_lba * physical_block_size,
            );
        }

        table.truncate(file_size);
        table
    }

    pub fn add_segment(&mut self, virtual_start: u64, byte_count: u64, physical_start: u64) {
        if byte_count == 0 {
            return;
        }

        self.mappings
            .push(ByteMapping::new(virtual_start, byte_count, physical_start));
        self.total_bytes = self.total_bytes.max(virtual_start + byte_count);
    }

    pub fn truncate(&mut self, total_bytes: u64) {
        self.total_bytes = total_bytes;
        self.mappings
            .retain(|m| m.virtual_range.start < total_bytes);

        for mapping in &mut self.mappings {
            if mapping.virtual_range.end > total_bytes {
                mapping.virtual_range.end = total_bytes;
            }
        }
    }

    pub fn set_total_bytes(&mut self, total_bytes: u64) {
        self.total_bytes = total_bytes;
    }

    pub fn optimize(&mut self) {
        if self.mappings.len() <= 1 {
            return;
        }

        self.mappings.sort_by_key(|m| m.virtual_range.start);

        let mut optimized = Vec::new();
        let mut current = self.mappings[0].clone();

        for mapping in self.mappings.iter().skip(1) {
            if current.virtual_range.end == mapping.virtual_range.start
                && current.physical_start + current.byte_count() == mapping.physical_start
            {
                current.virtual_range.end = mapping.virtual_range.end;
            } else {
                optimized.push(current);
                current = mapping.clone();
            }
        }

        optimized.push(current);
        self.mappings = optimized;
    }

    pub fn translate(&self, virtual_offset: u64) -> Option<u64> {
        for mapping in &self.mappings {
            if let Some(physical) = mapping.translate(virtual_offset) {
                return Some(physical);
            }
        }
        None
    }

    pub fn translate_range(&self, virtual_start: u64, byte_count: u64) -> Option<Vec<(u64, u64)>> {
        let end = virtual_start.checked_add(byte_count)?;
        if end > self.total_bytes {
            return None;
        }

        let mut result = Vec::new();
        let mut remaining = byte_count;
        let mut current_virtual = virtual_start;

        while remaining > 0 {
            let mapping = self
                .mappings
                .iter()
                .find(|mapping| mapping.virtual_range.contains(&current_virtual))?;
            let physical = mapping.translate(current_virtual)?;
            let contiguous_count = remaining.min(mapping.virtual_range.end - current_virtual);

            if let Some((last_physical, last_count)) = result.last_mut() {
                if *last_physical + *last_count == physical {
                    *last_count += contiguous_count;
                } else {
                    result.push((physical, contiguous_count));
                }
            } else {
                result.push((physical, contiguous_count));
            }
            current_virtual += contiguous_count;
            remaining -= contiguous_count;
        }

        Some(result)
    }

    pub fn translate_range_sparse(
        &self,
        virtual_start: u64,
        byte_count: u64,
    ) -> Option<Vec<(u64, u64, u64)>> {
        let end = virtual_start.checked_add(byte_count)?;
        if end > self.total_bytes {
            return None;
        }

        let mut result = Vec::new();
        let mut current_virtual = virtual_start;

        while current_virtual < end {
            if let Some(mapping) = self
                .mappings
                .iter()
                .find(|mapping| mapping.virtual_range.contains(&current_virtual))
            {
                let physical = mapping.translate(current_virtual)?;
                let contiguous_count =
                    (end - current_virtual).min(mapping.virtual_range.end - current_virtual);
                let dst_offset = current_virtual - virtual_start;

                if let Some((last_dst, last_physical, last_count)) = result.last_mut() {
                    if *last_dst + *last_count == dst_offset
                        && *last_physical + *last_count == physical
                    {
                        *last_count += contiguous_count;
                    } else {
                        result.push((dst_offset, physical, contiguous_count));
                    }
                } else {
                    result.push((dst_offset, physical, contiguous_count));
                }

                current_virtual += contiguous_count;
            } else {
                let next_mapped_start = self
                    .mappings
                    .iter()
                    .filter(|mapping| mapping.virtual_range.start > current_virtual)
                    .map(|mapping| mapping.virtual_range.start)
                    .min()
                    .unwrap_or(end);
                current_virtual = next_mapped_start.min(end);
            }
        }

        Some(result)
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn segment_count(&self) -> usize {
        self.mappings.len()
    }

    pub fn mappings(&self) -> &[ByteMapping] {
        &self.mappings
    }
}

impl Default for ByteMappingTable {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_mapping_from_file_extents() {
        let extents = [(0, 10, 2), (2, 20, 1)];
        let table = ByteMappingTable::from_file_extents(&extents, 1536, 512);

        assert_eq!(table.total_bytes(), 1536);
        assert_eq!(table.segment_count(), 2);
        assert_eq!(table.translate(0), Some(5120));
        assert_eq!(table.translate(1023), Some(6143));
        assert_eq!(table.translate(1024), Some(10240));
        assert_eq!(table.translate(1536), None);
    }

    #[test]
    fn test_byte_mapping_translate_range_preserves_segments() {
        let extents = [(0, 10, 1), (1, 20, 1)];
        let table = ByteMappingTable::from_file_extents(&extents, 1024, 512);
        let ranges = table.translate_range(0, 1024).expect("byte ranges");

        assert_eq!(ranges, alloc::vec![(5120, 512), (10240, 512)]);
    }

    #[test]
    fn test_byte_mapping_optimize_merges_adjacent_segments() {
        let mut table = ByteMappingTable::empty();
        table.add_segment(0, 512, 4096);
        table.add_segment(512, 512, 4608);

        assert_eq!(table.segment_count(), 2);
        table.optimize();
        assert_eq!(table.segment_count(), 1);
        assert_eq!(
            table.translate_range(0, 1024),
            Some(alloc::vec![(4096, 1024)])
        );
    }

    #[test]
    fn test_byte_mapping_sparse_translate_preserves_holes() {
        let mut table = ByteMappingTable::empty();
        table.add_segment(0, 512, 4096);
        table.add_segment(1024, 512, 8192);
        table.truncate(1536);

        assert_eq!(table.translate_range(0, 1536), None);
        assert_eq!(
            table.translate_range_sparse(0, 1536),
            Some(alloc::vec![(0, 4096, 512), (1024, 8192, 512)])
        );
    }
}
