use crate::FsError;
use alloc::rc::Rc;

/// Block IO 操作抽象
///
/// 用于解耦文件系统与具体 Block IO 实现
pub trait BlockIoOps {
    /// 获取块大小
    fn block_size(&self) -> u32;

    /// 获取总块数
    fn total_blocks(&self) -> u64;

    /// 读取块
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
}

/// Shared block device handle used by filesystem instances.
pub type SharedBlockIo = Rc<dyn BlockIoOps>;

impl<T: BlockIoOps + ?Sized> BlockIoOps for Rc<T> {
    fn block_size(&self) -> u32 {
        (**self).block_size()
    }

    fn total_blocks(&self) -> u64 {
        (**self).total_blocks()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        (**self).read_blocks(lba, buf)
    }
}

/// Validate and read one or more full hardware blocks.
pub fn read_full_blocks(
    block_io: &dyn BlockIoOps,
    lba: u64,
    buf: &mut [u8],
) -> Result<(), FsError> {
    let block_size = block_io.block_size() as usize;
    if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
        return Err(FsError::InvalidArgument);
    }

    let block_count = (buf.len() / block_size) as u64;
    if lba
        .checked_add(block_count)
        .map_or(true, |end| end > block_io.total_blocks())
    {
        return Err(FsError::ReadError);
    }

    block_io.read_blocks(lba, buf)
}

/// 动态分发的 Block IO
pub struct DynBlockIo {
    block_size: u32,
    total_blocks: u64,
    read_fn: fn(u64, &mut [u8]) -> Result<(), FsError>,
}

impl DynBlockIo {
    pub fn new(
        block_size: u32,
        total_blocks: u64,
        read_fn: fn(u64, &mut [u8]) -> Result<(), FsError>,
    ) -> Self {
        Self {
            block_size,
            total_blocks,
            read_fn,
        }
    }
}

impl BlockIoOps for DynBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        (self.read_fn)(lba, buf)
    }
}
