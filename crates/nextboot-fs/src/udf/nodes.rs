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
}
