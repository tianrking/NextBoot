use alloc::vec::Vec;
use core::ptr::NonNull;
use nextboot_fs::{BlockIoOps, FsError};
use nextboot_virtio::{PhysicalReader, VirtIoError, VirtualBlockIo};
use uefi::proto::media::block::BlockIO;

pub(super) fn alloc_buffer_for_block(block_size: u32) -> Result<Vec<u8>, FsError> {
    let len = usize::try_from(block_size).map_err(|_| FsError::InvalidArgument)?;
    if len == 0 {
        return Err(FsError::InvalidArgument);
    }

    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|_| FsError::OutOfMemory)?;
    buf.resize(len, 0);
    Ok(buf)
}

pub(super) struct UefiBlockIo {
    block_io: NonNull<BlockIO>,
    media_id: u32,
    block_size: u32,
    total_blocks: u64,
}

impl UefiBlockIo {
    pub(super) fn new(block_io: &BlockIO) -> Option<Self> {
        let media = block_io.media();
        let block_size = media.block_size();
        if block_size == 0 {
            return None;
        }

        Some(Self {
            block_io: NonNull::from(block_io),
            media_id: media.media_id(),
            block_size,
            total_blocks: media.last_block() + 1,
        })
    }
}

impl BlockIoOps for UefiBlockIo {
    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(FsError::InvalidArgument);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(FsError::ReadError);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| FsError::ReadError)
    }
}

impl PhysicalReader for UefiBlockIo {
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), VirtIoError> {
        let block_size = self.block_size as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(VirtIoError::InvalidBufferSize);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(VirtIoError::OutOfBounds);
        }

        let block_io = unsafe { self.block_io.as_ref() };
        block_io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| VirtIoError::ReadFailed)
    }
}

pub(super) struct PartitionBlockIo {
    parent: nextboot_fs::SharedBlockIo,
    start_lba: u64,
    total_blocks: u64,
}

impl PartitionBlockIo {
    pub(super) fn new(
        parent: nextboot_fs::SharedBlockIo,
        start_lba: u64,
        total_blocks: u64,
    ) -> Self {
        Self {
            parent,
            start_lba,
            total_blocks,
        }
    }
}

impl BlockIoOps for PartitionBlockIo {
    fn block_size(&self) -> u32 {
        self.parent.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let block_size = self.block_size() as usize;
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(FsError::InvalidArgument);
        }

        let block_count = (buf.len() / block_size) as u64;
        if lba
            .checked_add(block_count)
            .map_or(true, |end| end > self.total_blocks)
        {
            return Err(FsError::ReadError);
        }

        let parent_lba = self.start_lba.checked_add(lba).ok_or(FsError::ReadError)?;
        self.parent.read_blocks(parent_lba, buf)
    }
}

pub(super) struct VirtualIsoBlockIo {
    vbio: VirtualBlockIo,
    media_id: u32,
}

impl VirtualIsoBlockIo {
    pub(super) fn new(vbio: VirtualBlockIo) -> Self {
        let media_id = vbio.media_id();
        Self { vbio, media_id }
    }
}

impl BlockIoOps for VirtualIsoBlockIo {
    fn block_size(&self) -> u32 {
        self.vbio.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.vbio.block_count()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.vbio
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| FsError::ReadError)
    }
}
