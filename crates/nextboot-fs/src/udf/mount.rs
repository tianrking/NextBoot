use super::*;

impl Udf {
    pub(super) fn mount(&mut self) -> Result<(), FsError> {
        let anchor = self.read_anchor()?;
        self.read_volume_descriptor_sequence(anchor)?;

        if self.partition_maps.is_empty() || self.partitions.is_empty() {
            return Err(FsError::InvalidSignature);
        }

        let fileset_lba = self.map_logical_block(self.root_icb.block)?;
        let fileset = self.read_logical_block(fileset_lba)?;
        if read_u16(&fileset, TAG_IDENT_OFFSET)? != TAG_IDENT_FSD {
            return Err(FsError::InvalidSignature);
        }

        self.root_icb = read_long_ad(&fileset, FSD_ROOT_ICB_OFFSET)?;
        Ok(())
    }
    pub(super) fn read_anchor(&self) -> Result<ExtentAd, FsError> {
        for &lba in AVDP_CANDIDATES {
            if lba >= self.block_io.total_blocks() {
                continue;
            }

            let block = self.read_logical_block(lba)?;
            if read_u16(&block, TAG_IDENT_OFFSET)? == TAG_IDENT_AVDP
                && read_u32(&block, TAG_LOCATION_OFFSET)? == lba as u32
            {
                return Ok(ExtentAd {
                    length: read_u32(&block, AVDP_MAIN_VDS_LENGTH_OFFSET)?,
                    start: read_u32(&block, AVDP_MAIN_VDS_START_OFFSET)?,
                });
            }
        }

        Err(FsError::InvalidSignature)
    }
    pub(super) fn read_volume_descriptor_sequence(
        &mut self,
        anchor: ExtentAd,
    ) -> Result<(), FsError> {
        let descriptor_count = ((u64::from(anchor.length) + u64::from(self.block_size) - 1)
            / u64::from(self.block_size))
        .max(1);
        let end = u64::from(anchor.start)
            .checked_add(descriptor_count)
            .ok_or(FsError::Corrupted)?;
        let mut block_lba = u64::from(anchor.start);

        while block_lba < end {
            let block = self.read_logical_block(block_lba)?;
            match read_u16(&block, TAG_IDENT_OFFSET)? {
                TAG_IDENT_PD => self.read_partition_descriptor(&block, block_lba)?,
                TAG_IDENT_LVD => self.read_logical_volume_descriptor(&block)?,
                TAG_IDENT_TD => break,
                ident if ident > TAG_IDENT_TD => return Err(FsError::InvalidSignature),
                _ => {}
            }
            block_lba += 1;
        }

        self.resolve_partition_maps()
    }
    pub(super) fn read_partition_descriptor(
        &mut self,
        block: &[u8],
        descriptor_lba: u64,
    ) -> Result<(), FsError> {
        let partition = Partition {
            number: read_u16(block, PD_PARTITION_NUMBER_OFFSET)?,
            start: read_u32(block, PD_PARTITION_START_OFFSET)?,
            length: read_u32(block, PD_PARTITION_LENGTH_OFFSET)?,
            descriptor_lba,
        };
        self.partitions
            .try_reserve_exact(1)
            .map_err(|_| FsError::OutOfMemory)?;
        self.partitions.push(partition);
        Ok(())
    }
    pub(super) fn read_logical_volume_descriptor(&mut self, block: &[u8]) -> Result<(), FsError> {
        let logical_block_size = read_u32(block, LVD_BLOCK_SIZE_OFFSET)?;
        if logical_block_size == 0 || logical_block_size != self.block_size {
            return Err(FsError::BlockSizeMismatch);
        }
        self.logical_block_size = logical_block_size;
        self.root_icb = read_long_ad(block, LVD_ROOT_FILESET_OFFSET)?;

        let map_table_len = read_u32(block, LVD_MAP_TABLE_LENGTH_OFFSET)? as usize;
        let map_count = read_u32(block, LVD_NUM_PARTITION_MAPS_OFFSET)? as usize;
        let maps_end = LVD_PARTITION_MAPS_OFFSET
            .checked_add(map_table_len)
            .ok_or(FsError::Corrupted)?;
        if maps_end > block.len() {
            return Err(FsError::Corrupted);
        }

        self.partition_maps.clear();
        self.partition_maps
            .try_reserve_exact(map_count)
            .map_err(|_| FsError::OutOfMemory)?;

        let mut offset = LVD_PARTITION_MAPS_OFFSET;
        for _ in 0..map_count {
            if offset + 6 > maps_end {
                return Err(FsError::Corrupted);
            }

            let map_type = block[offset];
            let map_len = block[offset + 1] as usize;
            if map_type != 1 || map_len < 6 || offset + map_len > maps_end {
                return Err(FsError::UnsupportedFs);
            }

            let partition_number = read_u16(block, offset + 4)?;
            self.partition_maps.push(PartitionMap {
                partition_index: partition_number as usize,
            });
            offset += map_len;
        }

        Ok(())
    }
    pub(super) fn resolve_partition_maps(&mut self) -> Result<(), FsError> {
        for map in &mut self.partition_maps {
            let partition_number = map.partition_index as u16;
            let Some(index) = self
                .partitions
                .iter()
                .position(|partition| partition.number == partition_number)
            else {
                return Err(FsError::Corrupted);
            };
            map.partition_index = index;
        }
        Ok(())
    }
}
