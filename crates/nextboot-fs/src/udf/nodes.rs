use super::*;

impl Udf {
    pub(super) fn find_node(&self, path: &str) -> Result<UdfNode, FsError> {
        let mut node = self.read_icb(self.root_icb)?;
        let parts = path.split('/').filter(|part| !part.is_empty());

        for part in parts {
            if !node.is_dir() {
                return Err(FsError::NotDirectory);
            }

            let mut found = None;
            for entry in self.read_dir_entries(&node)? {
                if entry.name.eq_ignore_ascii_case(part) {
                    found = Some(entry.icb);
                    break;
                }
            }

            let Some(icb) = found else {
                return Err(FsError::FileNotFound);
            };
            node = self.read_icb(icb)?;
        }

        Ok(node)
    }

    /// Build a replacement patch for `path` so that its file entry points at
    /// `replacement_lba` with `replacement_size` visible bytes.
    pub fn file_replacement_patch(
        &self,
        path: &str,
        replacement_lba: u64,
        replacement_size: u64,
        allocated_bytes: u64,
    ) -> Result<UdfFileReplacementPatch, FsError> {
        let node = self.find_node(path)?;
        if !node.is_file() {
            return Err(FsError::NotFile);
        }

        let descriptor_size = match node.flags & ICB_AD_MASK {
            ICB_AD_SHORT | ICB_AD_IN_ICB => 8usize,
            ICB_AD_LONG => 16usize,
            ICB_AD_EXTENDED => return Err(FsError::UnsupportedFs),
            _ => return Err(FsError::UnsupportedFs),
        };

        if replacement_size > u64::from(EXTENT_LENGTH_MASK) {
            return Err(FsError::FileTooLarge);
        }

        let map = self
            .partition_maps
            .get(node.part_ref as usize)
            .ok_or(FsError::Corrupted)?;
        let partition = *self
            .partitions
            .get(map.partition_index)
            .ok_or(FsError::Corrupted)?;
        let replacement_block = replacement_lba
            .checked_sub(u64::from(partition.start))
            .ok_or(FsError::InvalidArgument)?;
        let replacement_block_u32 =
            u32::try_from(replacement_block).map_err(|_| FsError::FileTooLarge)?;
        let replacement_size_u32 =
            u32::try_from(replacement_size).map_err(|_| FsError::FileTooLarge)?;

        let mut entry = node.entry.clone();
        write_u64(&mut entry, FILE_ENTRY_FILE_SIZE_OFFSET, replacement_size)?;
        if node.tag_ident == TAG_IDENT_EFE {
            write_u64(&mut entry, EFE_OBJECT_SIZE_OFFSET, replacement_size)?;
            write_u64(&mut entry, EFE_BLOCKS_RECORDED_OFFSET, allocated_bytes)?;
        } else {
            write_u64(&mut entry, FE_BLOCKS_RECORDED_OFFSET, allocated_bytes)?;
        }
        let alloc_len_offset = if node.tag_ident == TAG_IDENT_FE {
            FE_ALLOC_DESCS_LENGTH_OFFSET
        } else {
            EFE_ALLOC_DESCS_LENGTH_OFFSET
        };
        write_u32(&mut entry, alloc_len_offset, descriptor_size as u32)?;

        let flags = (node.flags & !ICB_AD_MASK)
            | if descriptor_size == 16 {
                ICB_AD_LONG
            } else {
                ICB_AD_SHORT
            };
        write_u16(&mut entry, FILE_ENTRY_ICB_FLAGS_OFFSET, flags)?;

        let clear_len = node.alloc_desc_len.max(descriptor_size);
        let clear_end = node
            .alloc_desc_offset
            .checked_add(clear_len)
            .ok_or(FsError::Corrupted)?;
        if clear_end > entry.len() {
            return Err(FsError::Corrupted);
        }
        entry[node.alloc_desc_offset..clear_end].fill(0);
        write_u32(&mut entry, node.alloc_desc_offset, replacement_size_u32)?;
        write_u32(
            &mut entry,
            node.alloc_desc_offset + 4,
            replacement_block_u32,
        )?;
        if descriptor_size == 16 {
            write_u16(&mut entry, node.alloc_desc_offset + 8, node.part_ref)?;
        }
        refresh_descriptor_tag(&mut entry)?;

        let allocated_blocks = div_round_up(allocated_bytes, u64::from(self.logical_block_size));
        let replacement_end_lba = replacement_lba
            .checked_add(allocated_blocks)
            .ok_or(FsError::Corrupted)?;
        let partition_descriptor =
            self.partition_descriptor_patch(partition, replacement_end_lba)?;

        Ok(UdfFileReplacementPatch {
            file_entry_offset: node
                .entry_lba
                .checked_mul(u64::from(self.logical_block_size))
                .ok_or(FsError::Corrupted)?,
            file_entry_data: entry,
            partition_descriptor,
        })
    }
    pub(super) fn read_dir_node(&self, node: &UdfNode) -> Result<Vec<FileInfo>, FsError> {
        let dir_entries = self.read_dir_entries(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(dir_entries.len())
            .map_err(|_| FsError::OutOfMemory)?;

        for entry in dir_entries {
            let child = self.read_icb(entry.icb)?;
            let mut info = FileInfo::new(
                entry.name,
                child.file_size,
                entry.is_dir || child.is_dir(),
                self.node_start_lba(&child).unwrap_or(0),
            );
            info.contiguous = self.node_is_contiguous(&child);
            if info.is_dir {
                info.attributes |= FileAttributes::DIRECTORY;
            }
            if entry.hidden {
                info.attributes |= FileAttributes::HIDDEN;
            }
            out.push(info);
        }

        Ok(out)
    }
    pub(super) fn read_dir_entries(&self, node: &UdfNode) -> Result<Vec<UdfDirEntry>, FsError> {
        let size = usize::try_from(node.file_size).map_err(|_| FsError::FileTooLarge)?;
        let mut data = alloc_buffer(size)?;
        if size != 0 {
            self.read_node_data(node, 0, &mut data)?;
        }

        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < data.len() {
            if offset + FID_HEADER_SIZE > data.len() {
                break;
            }
            if read_u16(&data, offset + TAG_IDENT_OFFSET)? != TAG_IDENT_FID {
                return Err(FsError::Corrupted);
            }

            let characteristics = data[offset + FID_CHARACTERISTICS_OFFSET];
            let name_len = data[offset + FID_NAME_LENGTH_OFFSET] as usize;
            let icb = read_long_ad(&data, offset + FID_ICB_OFFSET)?;
            let imp_use_len = read_u16(&data, offset + FID_IMP_USE_LENGTH_OFFSET)? as usize;
            let name_offset = offset
                .checked_add(FID_HEADER_SIZE)
                .and_then(|value| value.checked_add(imp_use_len))
                .ok_or(FsError::Corrupted)?;
            let name_end = name_offset
                .checked_add(name_len)
                .ok_or(FsError::Corrupted)?;
            if name_end > data.len() {
                return Err(FsError::Corrupted);
            }

            if characteristics & (FID_CHAR_DELETED | FID_CHAR_PARENT) == 0 {
                let name = decode_osta_name(&data[name_offset..name_end])?;
                entries
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                entries.push(UdfDirEntry {
                    name,
                    icb,
                    is_dir: characteristics & FID_CHAR_DIRECTORY != 0,
                    hidden: characteristics & FID_CHAR_HIDDEN != 0,
                });
            }

            offset = align_up(name_end, 4).ok_or(FsError::Corrupted)?;
        }

        Ok(entries)
    }
    pub(super) fn read_icb(&self, icb: LongAd) -> Result<UdfNode, FsError> {
        let lba = self.map_logical_block(icb.block)?;
        let entry = self.read_logical_block(lba)?;
        let tag_ident = read_u16(&entry, TAG_IDENT_OFFSET)?;
        if tag_ident != TAG_IDENT_FE && tag_ident != TAG_IDENT_EFE {
            return Err(FsError::Corrupted);
        }

        let (ext_attr_offset, alloc_len_offset, alloc_offset) = if tag_ident == TAG_IDENT_FE {
            (
                FE_EXT_ATTR_LENGTH_OFFSET,
                FE_ALLOC_DESCS_LENGTH_OFFSET,
                FE_ALLOC_DESCS_OFFSET,
            )
        } else {
            (
                EFE_EXT_ATTR_LENGTH_OFFSET,
                EFE_ALLOC_DESCS_LENGTH_OFFSET,
                EFE_ALLOC_DESCS_OFFSET,
            )
        };
        let ext_attr_len = read_u32(&entry, ext_attr_offset)? as usize;
        let alloc_desc_len = read_u32(&entry, alloc_len_offset)? as usize;
        let alloc_desc_offset = alloc_offset
            .checked_add(ext_attr_len)
            .ok_or(FsError::Corrupted)?;
        if alloc_desc_offset
            .checked_add(alloc_desc_len)
            .map_or(true, |end| end > entry.len())
        {
            return Err(FsError::Corrupted);
        }

        Ok(UdfNode {
            entry_lba: lba,
            tag_ident,
            part_ref: icb.block.part_ref,
            file_type: *entry
                .get(FILE_ENTRY_ICB_FILE_TYPE_OFFSET)
                .ok_or(FsError::Corrupted)?,
            flags: read_u16(&entry, FILE_ENTRY_ICB_FLAGS_OFFSET)?,
            file_size: read_u64(&entry, FILE_ENTRY_FILE_SIZE_OFFSET)?,
            alloc_desc_offset,
            alloc_desc_len,
            entry,
        })
    }
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
