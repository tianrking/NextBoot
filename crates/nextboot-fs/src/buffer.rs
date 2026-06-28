use crate::FsError;
use alloc::vec::Vec;

/// 全局分配器辅助函数
pub fn alloc_buffer(size: usize) -> Result<Vec<u8>, FsError> {
    let mut buf = Vec::new();
    buf.try_reserve(size).map_err(|_| FsError::OutOfMemory)?;
    buf.resize(size, 0);
    Ok(buf)
}
