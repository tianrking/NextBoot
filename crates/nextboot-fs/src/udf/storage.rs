use super::*;

impl Udf {
    pub(super) fn read_node_data(
        &self,
        node: &UdfNode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= node.file_size || buf.is_empty() {
            return Ok(0);
        }

        let readable = buf
            .len()
            .min(usize::try_from(node.file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        if node.flags & ICB_AD_MASK == ICB_AD_IN_ICB {
            let start = node
                .alloc_desc_offset
                .checked_add(usize::try_from(offset).map_err(|_| FsError::FileTooLarge)?)
                .ok_or(FsError::Corrupted)?;
            let end = start.checked_add(readable).ok_or(FsError::Corrupted)?;
            if end > node.entry.len() {
                return Err(FsError::Corrupted);
            }
            buf[..readable].copy_from_slice(&node.entry[start..end]);
            return Ok(readable);
        }

        let extents = self.node_data_extents(node)?;
        let mut copied = 0usize;
        let mut file_cursor = 0u64;
        let block_size = u64::from(self.logical_block_size);

        for extent in extents {
            if extent.length == 0 || extent.extent_type != 0 {
                file_cursor = file_cursor.saturating_add(u64::from(extent.length));
                continue;
            }

            let extent_len = u64::from(extent.length);
            let extent_end = file_cursor.saturating_add(extent_len);
            if offset >= extent_end {
                file_cursor = extent_end;
                continue;
            }

            let within_extent = offset.saturating_sub(file_cursor);
            let lba_offset = within_extent / block_size;
            let in_block_offset = (within_extent % block_size) as usize;
            let mut current_lba = extent.physical_lba.saturating_add(lba_offset);
            let mut extent_remaining = (extent_len - within_extent) as usize;
            let mut block = alloc_buffer(self.logical_block_size as usize)?;

            while copied < readable && extent_remaining > 0 {
                self.read_full_logical_block(current_lba, &mut block)?;
                let source_offset = if lba_offset == 0 && copied == 0 {
                    in_block_offset
                } else {
                    0
                };
                let available = block
                    .len()
                    .saturating_sub(source_offset)
                    .min(extent_remaining);
                let to_copy = available.min(readable - copied);
                buf[copied..copied + to_copy]
                    .copy_from_slice(&block[source_offset..source_offset + to_copy]);
                copied += to_copy;
                extent_remaining -= to_copy;
                current_lba = current_lba.saturating_add(1);
            }

            if copied >= readable {
                break;
            }
            file_cursor = extent_end;
        }

        Ok(copied)
    }

    pub(super) fn node_extents(&self, node: &UdfNode) -> Result<Vec<FileExtent>, FsError> {
        let data_extents = self.node_data_extents(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(data_extents.len())
            .map_err(|_| FsError::OutOfMemory)?;

        let mut virtual_block_start = 0u64;
        for extent in data_extents {
            if extent.extent_type != 0 {
                virtual_block_start = virtual_block_start.saturating_add(div_round_up(
                    u64::from(extent.length),
                    u64::from(self.logical_block_size),
                ));
                continue;
            }

            let block_count =
                div_round_up(u64::from(extent.length), u64::from(self.logical_block_size));
            out.push(FileExtent::new(
                virtual_block_start,
                extent.physical_lba,
                block_count,
            ));
            virtual_block_start = virtual_block_start.saturating_add(block_count);
        }

        Ok(out)
    }

    pub(super) fn node_data_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        match node.flags & ICB_AD_MASK {
            ICB_AD_SHORT => self.short_extents(node),
            ICB_AD_LONG => self.long_extents(node),
            ICB_AD_IN_ICB => Ok(Vec::new()),
            ICB_AD_EXTENDED => Err(FsError::UnsupportedFs),
            _ => Err(FsError::UnsupportedFs),
        }
    }

    pub(super) fn short_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        let descriptors =
            &node.entry[node.alloc_desc_offset..node.alloc_desc_offset + node.alloc_desc_len];
        let mut extents = Vec::new();
        for chunk in descriptors.chunks_exact(8) {
            let raw_length = read_u32(chunk, 0)?;
            let length = raw_length & EXTENT_LENGTH_MASK;
            if length == 0 {
                continue;
            }
            extents
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extents.push(NodeExtent {
                length,
                physical_lba: self.map_partition_block(node.part_ref, read_u32(chunk, 4)?)?,
                extent_type: raw_length & EXTENT_TYPE_MASK,
            });
        }
        Ok(extents)
    }

    pub(super) fn long_extents(&self, node: &UdfNode) -> Result<Vec<NodeExtent>, FsError> {
        let descriptors =
            &node.entry[node.alloc_desc_offset..node.alloc_desc_offset + node.alloc_desc_len];
        let mut extents = Vec::new();
        for chunk in descriptors.chunks_exact(16) {
            let raw_length = read_u32(chunk, 0)?;
            let length = raw_length & EXTENT_LENGTH_MASK;
            if length == 0 {
                continue;
            }
            let address = LogicalBlockAddress {
                block_num: read_u32(chunk, 4)?,
                part_ref: read_u16(chunk, 8)?,
            };
            extents
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extents.push(NodeExtent {
                length,
                physical_lba: self.map_logical_block(address)?,
                extent_type: raw_length & EXTENT_TYPE_MASK,
            });
        }
        Ok(extents)
    }

    pub(super) fn node_start_lba(&self, node: &UdfNode) -> Option<u64> {
        self.node_extents(node)
            .ok()
            .and_then(|extents| extents.first().copied())
            .map(|extent| extent.physical_lba)
    }

    pub(super) fn node_is_contiguous(&self, node: &UdfNode) -> bool {
        self.node_extents(node)
            .map(|extents| extents.len() <= 1)
            .unwrap_or(false)
    }

    pub(super) fn map_logical_block(&self, address: LogicalBlockAddress) -> Result<u64, FsError> {
        self.map_partition_block(address.part_ref, address.block_num)
    }

    pub(super) fn map_partition_block(
        &self,
        part_ref: u16,
        block_num: u32,
    ) -> Result<u64, FsError> {
        let map = self
            .partition_maps
            .get(part_ref as usize)
            .ok_or(FsError::Corrupted)?;
        let partition = self
            .partitions
            .get(map.partition_index)
            .ok_or(FsError::Corrupted)?;
        if block_num >= partition.length {
            return Err(FsError::ReadError);
        }
        Ok(u64::from(partition.start) + u64::from(block_num))
    }

    pub(super) fn read_logical_block(&self, lba: u64) -> Result<Vec<u8>, FsError> {
        let mut block = alloc_buffer(self.logical_block_size as usize)?;
        self.read_full_logical_block(lba, &mut block)?;
        Ok(block)
    }

    pub(super) fn read_full_logical_block(
        &self,
        lba: u64,
        block: &mut [u8],
    ) -> Result<(), FsError> {
        if block.len() != self.logical_block_size as usize {
            return Err(FsError::InvalidArgument);
        }
        if self.logical_block_size != self.block_size {
            return Err(FsError::BlockSizeMismatch);
        }
        self.block_io.read_blocks(lba, block)
    }

    pub(super) fn partition_descriptor_patch(
        &self,
        partition: Partition,
        replacement_end_lba: u64,
    ) -> Result<Option<UdfPartitionDescriptorPatch>, FsError> {
        let partition_end = u64::from(partition.start)
            .checked_add(u64::from(partition.length))
            .ok_or(FsError::Corrupted)?;
        if replacement_end_lba <= partition_end {
            return Ok(None);
        }

        let new_length = replacement_end_lba
            .checked_sub(u64::from(partition.start))
            .ok_or(FsError::Corrupted)?;
        let new_length = u32::try_from(new_length).map_err(|_| FsError::FileTooLarge)?;
        let mut descriptor = self.read_logical_block(partition.descriptor_lba)?;
        write_u32(&mut descriptor, PD_PARTITION_LENGTH_OFFSET, new_length)?;
        refresh_descriptor_tag(&mut descriptor)?;

        Ok(Some(UdfPartitionDescriptorPatch {
            descriptor_offset: partition
                .descriptor_lba
                .checked_mul(u64::from(self.logical_block_size))
                .ok_or(FsError::Corrupted)?,
            descriptor_data: descriptor,
        }))
    }
}
