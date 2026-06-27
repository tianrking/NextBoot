use super::*;

impl VirtualBlockIoProtocol {
    fn read_blocks_status(
        &self,
        media_id: u32,
        lba: u64,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> UefiStatus {
        if buffer_size == 0 {
            return UefiStatus::Success;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter;
        }

        let block_size = self.media.block_size;
        if block_size == 0 || buffer_size % block_size as usize != 0 {
            return UefiStatus::BadBufferSize;
        }

        let buf = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*self.vbio.get() };

        vbio.read_blocks(media_id, lba, buf)
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn write_blocks_status(
        &self,
        media_id: u32,
        lba: u64,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> UefiStatus {
        if buffer_size == 0 {
            return UefiStatus::Success;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter;
        }

        let buf = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*self.vbio.get() };

        vbio.write_blocks(media_id, lba, buf)
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn read_disk_status(
        &self,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> UefiStatus {
        if buffer_size == 0 {
            return UefiStatus::Success;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter;
        }

        let buf = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*self.vbio.get() };

        vbio.read_bytes(media_id, offset, buf)
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn write_disk_status(
        &self,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> UefiStatus {
        if buffer_size == 0 {
            return UefiStatus::Success;
        }
        if buffer.is_null() {
            return UefiStatus::InvalidParameter;
        }

        let buf = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), buffer_size) };
        let vbio = unsafe { &*self.vbio.get() };

        vbio.write_blocks(media_id, offset, buf)
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn flush_status(&self) -> UefiStatus {
        let vbio = unsafe { &*self.vbio.get() };
        vbio.flush()
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn reset_status(&self, extended: bool) -> UefiStatus {
        let vbio = unsafe { &*self.vbio.get() };
        vbio.reset(extended)
            .map(|_| UefiStatus::Success)
            .unwrap_or_else(UefiStatus::from)
    }

    fn finish_block_io_2(&self, token: *mut BlockIo2Token, status: UefiStatus) -> u64 {
        if token.is_null() {
            return status as u64;
        }

        let token = unsafe { &mut *token };
        token.transaction_status = status as u64;
        if !token.event.is_null()
            && matches!(status, UefiStatus::Success)
            && !self.signal_event(token.event)
        {
            token.transaction_status = UefiStatus::DeviceError as u64;
            return UefiStatus::DeviceError as u64;
        }

        status as u64
    }

    fn finish_disk_io_2(&self, token: *mut DiskIo2Token, status: UefiStatus) -> u64 {
        if token.is_null() {
            return status as u64;
        }

        let token = unsafe { &mut *token };
        token.transaction_status = status as u64;
        if !token.event.is_null()
            && matches!(status, UefiStatus::Success)
            && !self.signal_event(token.event)
        {
            token.transaction_status = UefiStatus::DeviceError as u64;
            return UefiStatus::DeviceError as u64;
        }

        status as u64
    }

    #[cfg(not(test))]
    fn signal_event(&self, event: *mut core::ffi::c_void) -> bool {
        if event.is_null() {
            return true;
        }

        let Some(event) = (unsafe { uefi::Event::from_ptr(event) }) else {
            return false;
        };
        let Some(bt) = core::ptr::NonNull::new(self.boot_services as *mut BootServices) else {
            return false;
        };

        unsafe { bt.as_ref() }.signal_event(&event).is_ok()
    }

    #[cfg(test)]
    fn signal_event(&self, event: *mut core::ffi::c_void) -> bool {
        event.is_null()
    }

    /// Reset 处理函数
    pub(super) extern "efiapi" fn reset_handler(this: *mut BlockIoProtocol, extended: bool) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        wrapper.reset_status(extended) as u64
    }

    /// ReadBlocks 处理函数
    pub(super) extern "efiapi" fn read_blocks_handler(
        this: *mut BlockIoProtocol,
        media_id: u32,
        lba: u64,
        buffer_size: u64,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer_size > usize::MAX as u64 {
            return UefiStatus::InvalidParameter as u64;
        }

        wrapper.read_blocks_status(media_id, lba, buffer_size as usize, buffer) as u64
    }

    /// WriteBlocks 处理函数
    pub(super) extern "efiapi" fn write_blocks_handler(
        this: *mut BlockIoProtocol,
        media_id: u32,
        lba: u64,
        buffer_size: u64,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        if buffer_size > usize::MAX as u64 {
            return UefiStatus::InvalidParameter as u64;
        }

        wrapper.write_blocks_status(media_id, lba, buffer_size as usize, buffer) as u64
    }

    /// ResetEx 处理函数
    pub(super) extern "efiapi" fn reset_2_handler(
        this: *mut BlockIo2Protocol,
        extended: bool,
    ) -> u64 {
        let Some(wrapper) = Self::from_block_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        wrapper.reset_status(extended) as u64
    }

    /// ReadBlocksEx 处理函数
    pub(super) extern "efiapi" fn read_blocks_ex_handler(
        this: *mut BlockIo2Protocol,
        media_id: u32,
        lba: u64,
        token: *mut BlockIo2Token,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_block_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.read_blocks_status(media_id, lba, buffer_size, buffer);
        wrapper.finish_block_io_2(token, status)
    }

    /// WriteBlocksEx 处理函数
    pub(super) extern "efiapi" fn write_blocks_ex_handler(
        this: *mut BlockIo2Protocol,
        media_id: u32,
        lba: u64,
        token: *mut BlockIo2Token,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_block_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.write_blocks_status(media_id, lba, buffer_size, buffer);
        wrapper.finish_block_io_2(token, status)
    }

    /// FlushBlocksEx 处理函数
    pub(super) extern "efiapi" fn flush_blocks_ex_handler(
        this: *mut BlockIo2Protocol,
        token: *mut BlockIo2Token,
    ) -> u64 {
        let Some(wrapper) = Self::from_block_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.flush_status();
        wrapper.finish_block_io_2(token, status)
    }

    /// ReadDisk 处理函数
    pub(super) extern "efiapi" fn read_disk_handler(
        this: *mut DiskIoProtocol,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        wrapper.read_disk_status(media_id, offset, buffer_size, buffer) as u64
    }

    /// WriteDisk 处理函数
    pub(super) extern "efiapi" fn write_disk_handler(
        this: *mut DiskIoProtocol,
        media_id: u32,
        offset: u64,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        wrapper.write_disk_status(media_id, offset, buffer_size, buffer) as u64
    }

    /// Cancel Disk IO 2 处理函数
    pub(super) extern "efiapi" fn cancel_disk_ex_handler(_this: *mut DiskIo2Protocol) -> u64 {
        UefiStatus::Success as u64
    }

    /// ReadDiskEx 处理函数
    pub(super) extern "efiapi" fn read_disk_ex_handler(
        this: *mut DiskIo2Protocol,
        media_id: u32,
        offset: u64,
        token: *mut DiskIo2Token,
        buffer_size: usize,
        buffer: *mut core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.read_disk_status(media_id, offset, buffer_size, buffer);
        wrapper.finish_disk_io_2(token, status)
    }

    /// WriteDiskEx 处理函数
    pub(super) extern "efiapi" fn write_disk_ex_handler(
        this: *mut DiskIo2Protocol,
        media_id: u32,
        offset: u64,
        token: *mut DiskIo2Token,
        buffer_size: usize,
        buffer: *const core::ffi::c_void,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.write_disk_status(media_id, offset, buffer_size, buffer);
        wrapper.finish_disk_io_2(token, status)
    }

    /// FlushDiskEx 处理函数
    pub(super) extern "efiapi" fn flush_disk_ex_handler(
        this: *mut DiskIo2Protocol,
        token: *mut DiskIo2Token,
    ) -> u64 {
        let Some(wrapper) = Self::from_disk_io_2_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        let status = wrapper.flush_status();
        wrapper.finish_disk_io_2(token, status)
    }

    /// Flush 处理函数
    pub(super) extern "efiapi" fn flush_handler(this: *mut BlockIoProtocol) -> u64 {
        let Some(wrapper) = Self::from_protocol(this) else {
            return UefiStatus::InvalidParameter as u64;
        };

        wrapper.flush_status() as u64
    }

    fn from_protocol(this: *mut BlockIoProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        // `protocol` is the first field and the type is repr(C), so both
        // pointers have the same address.
        Some(unsafe { &mut *(this.cast::<Self>()) })
    }

    fn from_block_io_2_protocol(this: *mut BlockIo2Protocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, block_io_2);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
    }

    fn from_disk_protocol(this: *mut DiskIoProtocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, disk_io);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
    }

    fn from_disk_io_2_protocol(this: *mut DiskIo2Protocol) -> Option<&'static mut Self> {
        if this.is_null() {
            return None;
        }

        let offset = core::mem::offset_of!(Self, disk_io_2);
        let ptr = unsafe { this.cast::<u8>().sub(offset).cast::<Self>() };
        Some(unsafe { &mut *ptr })
    }
}
