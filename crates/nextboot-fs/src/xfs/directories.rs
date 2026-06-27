use super::parse::{read_be_u16, read_be_u32, read_be_u64};
use super::{Xfs, XfsDirEntry, XfsNode, XFS_DINODE_FMT_LOCAL};
use crate::FsError;
use alloc::string::String;
use alloc::vec::Vec;

const NEXTBOOT_XFS_DIR_MAGIC: &[u8; 4] = b"NXD1";
const XFS_DIR2_BLOCK_MAGIC: u32 = 0x5844_4232;
const XFS_DIR3_BLOCK_MAGIC: u32 = 0x5844_4233;
const XFS_DIR2_DATA_MAGIC: u32 = 0x5844_4432;
const XFS_DIR3_DATA_MAGIC: u32 = 0x5844_4433;
const XFS_DIR2_DATA_FREE_TAG: u16 = 0xffff;
const XFS_DIR2_DATA_HDR_SIZE: usize = 16;
const XFS_DIR3_DATA_HDR_SIZE: usize = 64;
const XFS_DIR2_BLOCK_TAIL_SIZE: usize = 8;

pub(super) fn read_dir_entries(fs: &Xfs, node: &XfsNode) -> Result<Vec<XfsDirEntry>, FsError> {
    if node.format == XFS_DINODE_FMT_LOCAL {
        return read_shortform_dir(fs, node);
    }

    let mut entries = Vec::new();
    let mut parsed_known_block = false;
    for block in read_directory_blocks(fs, node)? {
        if block.get(0..4) == Some(NEXTBOOT_XFS_DIR_MAGIC) {
            parsed_known_block = true;
            read_nextboot_dir(&block, &mut entries)?;
            continue;
        }
        if read_dir2_block(&block, fs.has_ftype, &mut entries)? {
            parsed_known_block = true;
        }
    }
    if !parsed_known_block {
        return Err(FsError::UnsupportedFs);
    }
    Ok(entries)
}

fn read_directory_blocks(fs: &Xfs, node: &XfsNode) -> Result<Vec<Vec<u8>>, FsError> {
    let dir_blocks = u64::from(fs.dir_block_size / fs.block_size).max(1);
    let leaf_start = (1u64 << 35) / u64::from(fs.block_size);
    let mut out = Vec::new();
    for extent in &node.extents {
        if extent.file_block >= leaf_start {
            continue;
        }
        let mut remaining = u64::from(extent.block_count);
        let mut physical = extent.physical_block;
        while remaining > 0 {
            let chunk_blocks = remaining.min(dir_blocks);
            let mut chunk = Vec::new();
            let chunk_len = usize::try_from(chunk_blocks)
                .ok()
                .and_then(|count| count.checked_mul(fs.block_size as usize))
                .ok_or(FsError::FileTooLarge)?;
            chunk
                .try_reserve_exact(chunk_len)
                .map_err(|_| FsError::OutOfMemory)?;
            for index in 0..chunk_blocks {
                chunk.extend_from_slice(&fs.read_block(physical + index)?);
            }
            out.push(chunk);
            physical = physical.saturating_add(chunk_blocks);
            remaining -= chunk_blocks;
        }
    }
    Ok(out)
}

fn read_nextboot_dir(data: &[u8], entries: &mut Vec<XfsDirEntry>) -> Result<(), FsError> {
    let count = read_be_u16(data, 4)? as usize;
    let mut offset = 6usize;
    entries
        .try_reserve_exact(count)
        .map_err(|_| FsError::OutOfMemory)?;
    for _ in 0..count {
        let inode_number = read_be_u64(data, offset)?;
        let name_len = *data.get(offset + 8).ok_or(FsError::Corrupted)? as usize;
        offset = offset.checked_add(9).ok_or(FsError::Corrupted)?;
        let name_end = offset.checked_add(name_len).ok_or(FsError::Corrupted)?;
        let name = decode_name(data.get(offset..name_end).ok_or(FsError::Corrupted)?)?;
        entries.push(XfsDirEntry { inode_number, name });
        offset = name_end;
    }
    Ok(())
}

fn read_shortform_dir(fs: &Xfs, node: &XfsNode) -> Result<Vec<XfsDirEntry>, FsError> {
    let data = &node.local_data;
    let count = *data.first().ok_or(FsError::Corrupted)? as usize;
    let i8count = *data.get(1).ok_or(FsError::Corrupted)?;
    let inode_bytes = if i8count > 0 { 8 } else { 4 };
    let mut offset = 2usize.checked_add(inode_bytes).ok_or(FsError::Corrupted)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_| FsError::OutOfMemory)?;
    for _ in 0..count {
        let name_len = *data.get(offset).ok_or(FsError::Corrupted)? as usize;
        offset = offset.checked_add(3).ok_or(FsError::Corrupted)?;
        let name_end = offset.checked_add(name_len).ok_or(FsError::Corrupted)?;
        let name = decode_name(data.get(offset..name_end).ok_or(FsError::Corrupted)?)?;
        offset = name_end;
        if fs.has_ftype {
            offset = offset.checked_add(1).ok_or(FsError::Corrupted)?;
        }
        let inode_number = read_shortform_inode(data, offset, inode_bytes)?;
        offset = offset.checked_add(inode_bytes).ok_or(FsError::Corrupted)?;
        push_visible_entry(&mut entries, inode_number, name)?;
    }
    Ok(entries)
}

fn read_shortform_inode(data: &[u8], offset: usize, len: usize) -> Result<u64, FsError> {
    match len {
        4 => Ok(u64::from(read_be_u32(data, offset)?)),
        8 => read_be_u64(data, offset),
        _ => Err(FsError::Corrupted),
    }
}

fn read_dir2_block(
    data: &[u8],
    fs_has_ftype: bool,
    entries: &mut Vec<XfsDirEntry>,
) -> Result<bool, FsError> {
    let magic = read_be_u32(data, 0)?;
    let (header_size, has_ftype, data_limit) = match magic {
        XFS_DIR2_BLOCK_MAGIC => (
            XFS_DIR2_DATA_HDR_SIZE,
            fs_has_ftype,
            block_data_limit(data, false)?,
        ),
        XFS_DIR3_BLOCK_MAGIC => (XFS_DIR3_DATA_HDR_SIZE, true, block_data_limit(data, true)?),
        XFS_DIR2_DATA_MAGIC => (XFS_DIR2_DATA_HDR_SIZE, fs_has_ftype, data.len()),
        XFS_DIR3_DATA_MAGIC => (XFS_DIR3_DATA_HDR_SIZE, true, data.len()),
        _ => return Ok(false),
    };
    parse_dir2_entries(data, header_size, data_limit, has_ftype, entries)?;
    Ok(true)
}

fn block_data_limit(data: &[u8], is_v3: bool) -> Result<usize, FsError> {
    if data.len() < XFS_DIR2_BLOCK_TAIL_SIZE {
        return Err(FsError::Corrupted);
    }
    let tail = data.len() - XFS_DIR2_BLOCK_TAIL_SIZE;
    let count = read_be_u32(data, tail)? as usize;
    let leaf_entry_size = if is_v3 { 16 } else { 8 };
    tail.checked_sub(
        count
            .checked_mul(leaf_entry_size)
            .ok_or(FsError::Corrupted)?,
    )
    .ok_or(FsError::Corrupted)
}

fn parse_dir2_entries(
    data: &[u8],
    mut offset: usize,
    limit: usize,
    has_ftype: bool,
    entries: &mut Vec<XfsDirEntry>,
) -> Result<(), FsError> {
    while offset.checked_add(4).is_some_and(|end| end <= limit) {
        let marker = read_be_u16(data, offset)?;
        if marker == XFS_DIR2_DATA_FREE_TAG {
            let length = read_be_u16(data, offset + 2)? as usize;
            if length < 8 {
                return Err(FsError::Corrupted);
            }
            offset = offset.checked_add(length).ok_or(FsError::Corrupted)?;
            continue;
        }

        let inode_number = read_be_u64(data, offset)?;
        let name_len_offset = offset.checked_add(8).ok_or(FsError::Corrupted)?;
        let name_len = *data.get(name_len_offset).ok_or(FsError::Corrupted)? as usize;
        let name_start = name_len_offset.checked_add(1).ok_or(FsError::Corrupted)?;
        let name_end = name_start.checked_add(name_len).ok_or(FsError::Corrupted)?;
        let after_name = name_end
            .checked_add(usize::from(has_ftype))
            .ok_or(FsError::Corrupted)?;
        let record_end = align_up(after_name.checked_add(2).ok_or(FsError::Corrupted)?, 8)?;
        if record_end > limit {
            return Err(FsError::Corrupted);
        }
        let name = decode_name(data.get(name_start..name_end).ok_or(FsError::Corrupted)?)?;
        push_visible_entry(entries, inode_number, name)?;
        offset = record_end;
    }
    Ok(())
}

fn push_visible_entry(
    entries: &mut Vec<XfsDirEntry>,
    inode_number: u64,
    name: String,
) -> Result<(), FsError> {
    if name == "." || name == ".." {
        return Ok(());
    }
    entries
        .try_reserve_exact(1)
        .map_err(|_| FsError::OutOfMemory)?;
    entries.push(XfsDirEntry { inode_number, name });
    Ok(())
}

fn decode_name(data: &[u8]) -> Result<String, FsError> {
    String::from_utf8(data.to_vec()).map_err(|_| FsError::Corrupted)
}

fn align_up(value: usize, alignment: usize) -> Result<usize, FsError> {
    value
        .checked_add(alignment.checked_sub(1).ok_or(FsError::Corrupted)?)
        .map(|value| value / alignment * alignment)
        .ok_or(FsError::Corrupted)
}
