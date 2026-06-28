use super::parse::{read_be_u16, read_be_u32, read_be_u64};
use super::{
    Xfs, XfsExtent, XfsNode, INODE_FORMAT, INODE_MAGIC, INODE_MODE, INODE_NEXTENTS, INODE_SIZE,
    INODE_V2_DATA_FORK, INODE_V3_DATA_FORK, XFS_DINODE_FMT_EXTENTS, XFS_DINODE_FMT_LOCAL,
    XFS_DINODE_MAGIC,
};
use crate::FsError;
use alloc::vec::Vec;

impl Xfs {
    pub(super) fn read_inode(&self, inode_number: u64) -> Result<XfsNode, FsError> {
        if inode_number == 0 {
            return Err(FsError::Corrupted);
        }
        let block = self.read_inode_bytes(inode_number)?;
        if read_be_u16(&block, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        let format = block[INODE_FORMAT];
        if !matches!(format, XFS_DINODE_FMT_LOCAL | XFS_DINODE_FMT_EXTENTS) {
            return Err(FsError::UnsupportedFs);
        }
        let data_fork = if block[super::INODE_VERSION] >= 3 {
            INODE_V3_DATA_FORK
        } else {
            INODE_V2_DATA_FORK
        };
        if data_fork > self.inode_size as usize || data_fork > block.len() {
            return Err(FsError::Corrupted);
        }
        let size = read_be_u64(&block, INODE_SIZE)?;
        let mut local_data = Vec::new();
        if format == XFS_DINODE_FMT_LOCAL {
            let local_len = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
            let local_end = data_fork.checked_add(local_len).ok_or(FsError::Corrupted)?;
            if local_end > self.inode_size as usize || local_end > block.len() {
                return Err(FsError::Corrupted);
            }
            local_data
                .try_reserve_exact(local_len)
                .map_err(|_| FsError::OutOfMemory)?;
            local_data.extend_from_slice(&block[data_fork..local_end]);
        }
        let nextents = read_be_u32(&block, INODE_NEXTENTS)? as usize;
        if format == XFS_DINODE_FMT_LOCAL && nextents != 0 {
            return Err(FsError::Corrupted);
        }

        let mut extents = Vec::new();
        if format == XFS_DINODE_FMT_EXTENTS {
            let extent_end = data_fork
                .checked_add(nextents.checked_mul(16).ok_or(FsError::Corrupted)?)
                .ok_or(FsError::Corrupted)?;
            if extent_end > self.inode_size as usize || extent_end > block.len() {
                return Err(FsError::Corrupted);
            }
            extents
                .try_reserve_exact(nextents)
                .map_err(|_| FsError::OutOfMemory)?;
            for index in 0..nextents {
                let offset = data_fork + index * 16;
                extents.push(read_bmbt_record(&block[offset..offset + 16])?);
            }
        }
        Ok(XfsNode {
            inode_number,
            mode: read_be_u16(&block, INODE_MODE)?,
            size,
            format,
            extents,
            local_data,
        })
    }

    fn read_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        if let Ok(inode) = self.read_mapped_inode_bytes(inode_number) {
            return Ok(inode);
        }
        self.read_legacy_inode_bytes(inode_number)
    }

    fn read_mapped_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        if self.inode_agino_bits >= 64 {
            return Err(FsError::Corrupted);
        }
        let agino_mask = (1u64 << self.inode_agino_bits) - 1;
        let agno = inode_number >> self.inode_agino_bits;
        let agino = inode_number & agino_mask;
        let agbno = agino >> self.inopblog;
        let inode_index = agino & ((1u64 << self.inopblog) - 1);
        let fs_block = agno
            .checked_mul(u64::from(self.agblocks))
            .and_then(|base| base.checked_add(agbno))
            .ok_or(FsError::Corrupted)?;
        let block = self.read_block(fs_block)?;
        let start = usize::try_from(inode_index)
            .ok()
            .and_then(|index| index.checked_mul(self.inode_size as usize))
            .ok_or(FsError::Corrupted)?;
        let end = start
            .checked_add(self.inode_size as usize)
            .ok_or(FsError::Corrupted)?;
        let inode = block.get(start..end).ok_or(FsError::Corrupted)?.to_vec();
        if read_be_u16(&inode, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        Ok(inode)
    }

    fn read_legacy_inode_bytes(&self, inode_number: u64) -> Result<Vec<u8>, FsError> {
        let block = self.read_block(inode_number)?;
        let inode = block
            .get(..self.inode_size as usize)
            .ok_or(FsError::Corrupted)?
            .to_vec();
        if read_be_u16(&inode, INODE_MAGIC)? != XFS_DINODE_MAGIC {
            return Err(FsError::Corrupted);
        }
        Ok(inode)
    }
}

fn read_bmbt_record(data: &[u8]) -> Result<XfsExtent, FsError> {
    let l0 = read_be_u64(data, 0)?;
    let l1 = read_be_u64(data, 8)?;
    if l0 >> 63 != 0 {
        return Err(FsError::UnsupportedFs);
    }
    let file_block = (l0 >> 9) & ((1u64 << 54) - 1);
    let physical_block = ((l0 & 0x1FF) << 43) | (l1 >> 21);
    let block_count = (l1 & ((1u64 << 21) - 1)) as u32;
    if block_count == 0 {
        return Err(FsError::Corrupted);
    }
    Ok(XfsExtent {
        file_block,
        physical_block,
        block_count,
    })
}
