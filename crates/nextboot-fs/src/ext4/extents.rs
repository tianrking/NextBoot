use super::*;

impl Ext4 {
    pub(super) fn read_node_data(
        &self,
        node: &Ext4Node,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= node.size || buf.is_empty() {
            return Ok(0);
        }
        let readable = buf
            .len()
            .min(usize::try_from(node.size - offset).map_err(|_| FsError::FileTooLarge)?);
        let block_size = u64::from(self.block_size);
        let mut copied = 0usize;
        for extent in self.extents(node)? {
            let extent_start = u64::from(extent.file_block) * block_size;
            let extent_len = u64::from(extent.block_count) * block_size;
            let extent_end = extent_start.saturating_add(extent_len);
            if offset >= extent_end || offset + readable as u64 <= extent_start {
                continue;
            }
            let read_start = offset.max(extent_start);
            let read_end = (offset + readable as u64).min(extent_end);
            let first_block = (read_start - extent_start) / block_size;
            let last_block = (read_end - extent_start + block_size - 1) / block_size;
            let mut block_index = first_block;
            while block_index < last_block && copied < readable {
                let block = self.read_block(extent.physical_block + block_index)?;
                let block_file_offset = extent_start + block_index * block_size;
                let start = read_start.max(block_file_offset) - block_file_offset;
                let end = read_end.min(block_file_offset + block_size) - block_file_offset;
                let len = usize::try_from(end - start).map_err(|_| FsError::FileTooLarge)?;
                buf[copied..copied + len].copy_from_slice(&block[start as usize..end as usize]);
                copied += len;
                block_index += 1;
            }
        }
        Ok(copied)
    }

    pub(super) fn file_extents_for_node(
        &self,
        node: &Ext4Node,
    ) -> Result<Vec<FileExtent>, FsError> {
        let extents = self.extents(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(extents.len())
            .map_err(|_| FsError::OutOfMemory)?;
        for extent in extents {
            out.push(FileExtent::new(
                u64::from(extent.file_block),
                extent.physical_block,
                u64::from(extent.block_count),
            ));
        }
        Ok(out)
    }

    pub(super) fn extents(&self, node: &Ext4Node) -> Result<Vec<Ext4Extent>, FsError> {
        if node.flags & EXT4_EXTENTS_FL != 0 {
            return self.extent_tree_extents(node);
        }

        self.legacy_block_extents(node)
    }

    fn extent_tree_extents(&self, node: &Ext4Node) -> Result<Vec<Ext4Extent>, FsError> {
        let root = &node.inode[INODE_BLOCKS..INODE_BLOCKS + 60];
        if read_u16(root, 0)? != EXT4_EXTENT_MAGIC || read_u16(root, EXTENT_HEADER_DEPTH)? != 0 {
            return Err(FsError::UnsupportedFs);
        }
        let entries = read_u16(root, EXTENT_HEADER_ENTRIES)? as usize;
        if EXTENT_ENTRY_OFFSET + entries * EXTENT_ENTRY_SIZE > root.len() {
            return Err(FsError::Corrupted);
        }
        let mut out = Vec::new();
        for index in 0..entries {
            let offset = EXTENT_ENTRY_OFFSET + index * EXTENT_ENTRY_SIZE;
            let block_count = read_u16(root, offset + 4)? & 0x7FFF;
            if block_count == 0 {
                continue;
            }
            out.push(Ext4Extent {
                file_block: read_u32(root, offset)?,
                block_count,
                physical_block: (u64::from(read_u16(root, offset + 6)?) << 32)
                    | u64::from(read_u32(root, offset + 8)?),
            });
        }
        Ok(out)
    }

    fn legacy_block_extents(&self, node: &Ext4Node) -> Result<Vec<Ext4Extent>, FsError> {
        let needed_blocks = node.size.div_ceil(u64::from(self.block_size));
        if needed_blocks == 0 {
            return Ok(Vec::new());
        }
        let max_single_indirect = u64::from(self.block_size / 4);
        if needed_blocks > LEGACY_DIRECT_BLOCKS as u64 + max_single_indirect {
            return Err(FsError::UnsupportedFs);
        }

        let blocks = &node.inode[INODE_BLOCKS..INODE_BLOCKS + 60];
        let mut out = Vec::new();
        let mut file_block = 0u64;
        while file_block < needed_blocks && file_block < LEGACY_DIRECT_BLOCKS as u64 {
            let physical = read_u32(blocks, file_block as usize * 4)?;
            self.push_legacy_extent(&mut out, file_block, physical)?;
            file_block += 1;
        }

        if file_block < needed_blocks {
            let indirect = read_u32(blocks, LEGACY_SINGLE_INDIRECT_INDEX * 4)?;
            if indirect == 0 {
                return Err(FsError::Corrupted);
            }
            let block = self.read_block(u64::from(indirect))?;
            while file_block < needed_blocks {
                let index = file_block as usize - LEGACY_DIRECT_BLOCKS;
                let physical = read_u32(&block, index * 4)?;
                self.push_legacy_extent(&mut out, file_block, physical)?;
                file_block += 1;
            }
        }
        Ok(out)
    }

    fn push_legacy_extent(
        &self,
        out: &mut Vec<Ext4Extent>,
        file_block: u64,
        physical: u32,
    ) -> Result<(), FsError> {
        if physical == 0 || file_block > u64::from(u32::MAX) {
            return Err(FsError::Corrupted);
        }
        if let Some(last) = out.last_mut() {
            let expected = last.physical_block + u64::from(last.block_count);
            if expected == u64::from(physical) && last.block_count < u16::MAX {
                last.block_count += 1;
                return Ok(());
            }
        }
        out.push(Ext4Extent {
            file_block: file_block as u32,
            block_count: 1,
            physical_block: u64::from(physical),
        });
        Ok(())
    }
}
