use crate::{alloc_buffer, read_full_blocks, FileExtent, FsError};
use alloc::vec::Vec;

use super::fs::ExFat;

impl ExFat {
    pub(super) fn contiguous_extents(
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

        let block_count = (file_size + self.sector_size as u64 - 1) / self.sector_size as u64;
        extents.push(FileExtent::new(
            0,
            self.cluster_to_sector(start_cluster),
            block_count,
        ));
        Ok(extents)
    }

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

        let blocks_per_cluster = self.blocks_per_cluster();
        let mut blocks_remaining =
            (file_size + self.sector_size as u64 - 1) / self.sector_size as u64;
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
                self.cluster_to_sector(cluster),
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

    pub(super) fn read_from_extents(
        &self,
        extents: &[FileExtent],
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        let block_size = self.sector_size as u64;
        let mut skip_bytes = offset;
        let mut bytes_read = 0usize;
        let mut block_buf = alloc_buffer(self.sector_size as usize)?;

        for extent in extents {
            let extent_bytes = extent.block_count * block_size;
            if skip_bytes >= extent_bytes {
                skip_bytes -= extent_bytes;
                continue;
            }

            let mut extent_offset = skip_bytes;
            skip_bytes = 0;

            while bytes_read < buf.len() && extent_offset < extent_bytes {
                let block_offset = extent_offset / block_size;
                let in_block_offset = (extent_offset % block_size) as usize;
                let lba = extent.physical_lba + block_offset;

                read_full_blocks(self.block_io.as_ref(), lba, &mut block_buf)?;

                let available = block_buf.len() - in_block_offset;
                let needed = buf.len() - bytes_read;
                let copy_size = available.min(needed);

                buf[bytes_read..bytes_read + copy_size]
                    .copy_from_slice(&block_buf[in_block_offset..in_block_offset + copy_size]);

                bytes_read += copy_size;
                extent_offset += copy_size as u64;
            }

            if bytes_read == buf.len() {
                break;
            }
        }

        Ok(bytes_read)
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
