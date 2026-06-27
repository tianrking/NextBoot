use crate::{FileExtent, FsError};
use alloc::vec::Vec;

use super::fs::Fat32;

impl Fat32 {
    pub(super) fn cluster_chain_extents(
        &self,
        start_cluster: u32,
        file_size: u64,
    ) -> Result<Vec<FileExtent>, FsError> {
        let mut extents = Vec::new();
        if file_size == 0 {
            return Ok(extents);
        }

        if start_cluster < 2 || start_cluster >= self.total_clusters + 2 {
            return Err(FsError::Corrupted);
        }

        let blocks_per_cluster = self.sectors_per_cluster as u64;
        let mut blocks_remaining =
            (file_size + self.block_size as u64 - 1) / self.block_size as u64;
        let mut virtual_block = 0u64;
        let mut cluster = start_cluster;

        while blocks_remaining > 0 {
            if cluster < 2 || cluster >= self.total_clusters + 2 {
                return Err(FsError::Corrupted);
            }

            let block_count = blocks_per_cluster.min(blocks_remaining);
            push_extent(
                &mut extents,
                virtual_block,
                self.cluster_to_lba(cluster),
                block_count,
            );

            virtual_block += block_count;
            blocks_remaining -= block_count;

            if blocks_remaining > 0 {
                cluster = self.get_next_cluster(cluster)?;
                if self.is_end_of_chain(cluster) {
                    return Err(FsError::Corrupted);
                }
            }
        }

        Ok(extents)
    }
}

fn push_extent(
    extents: &mut Vec<FileExtent>,
    virtual_block_start: u64,
    physical_lba: u64,
    block_count: u64,
) {
    if block_count == 0 {
        return;
    }

    if let Some(last) = extents.last_mut() {
        if last.virtual_block_end() == virtual_block_start
            && last.physical_lba_end() == physical_lba
        {
            last.block_count += block_count;
            return;
        }
    }

    extents.push(FileExtent::new(
        virtual_block_start,
        physical_lba,
        block_count,
    ));
}
