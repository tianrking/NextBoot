use super::source_volume::{SourceVolumeFileMetadata, SourceVolumeReader};
use crate::scanner::{IsoExtent, IsoFile};
use crate::wim;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use nextboot_fs::BlockIoOps;
use nextboot_virtio::PhysicalReader;
use uefi::Status;

#[derive(Default)]
pub(super) struct WimbootInternalFiles {
    pub(super) bootmgfw: Option<WimbootRuntimeFile>,
    pub(super) bcd: Option<WimbootRuntimeFile>,
    pub(super) boot_sdi: Option<WimbootRuntimeFile>,
    pub(super) winpeshl: Option<Vec<u8>>,
}

pub(super) struct WimbootWimImage {
    pub(super) metadata: wim::WimMetadata,
    pub(super) lookup: Vec<u8>,
    pub(super) image_metadata: Vec<u8>,
}

pub(super) struct WimbootRuntimeContext {
    pub(super) reader: SourceVolumeReader,
    pub(super) files: Vec<WimbootRuntimeFile>,
}

impl WimbootRuntimeContext {
    pub(super) fn find_file(&self, path: &[u8]) -> Option<&WimbootRuntimeFile> {
        self.files
            .iter()
            .find(|file| file.callback_path.as_bytes() == path)
    }
}

#[derive(Clone)]
pub(super) struct WimbootRuntimeFile {
    callback_path: String,
    pub(super) size: u64,
    storage: WimbootRuntimeFileStorage,
}

#[derive(Clone)]
pub(super) struct WimbootMappedSegment {
    pub(super) virtual_offset: u64,
    pub(super) physical_offset: u64,
    pub(super) byte_count: u64,
}

#[derive(Clone)]
enum WimbootRuntimeFileStorage {
    Disk {
        block_size: u32,
        extents: Vec<IsoExtent>,
    },
    MappedBytes(Vec<WimbootMappedSegment>),
    Memory(Vec<u8>),
    WimResource {
        wim: Box<WimbootRuntimeFile>,
        metadata: wim::WimMetadata,
        resource: wim::WimResourceHeader,
    },
}

impl WimbootRuntimeFileStorage {
    fn read_range(&self, reader: &SourceVolumeReader, offset: u64, buf: &mut [u8]) -> Option<()> {
        match self {
            Self::Disk {
                block_size,
                extents,
            } => read_extent_range(reader, *block_size, extents, offset, buf),
            Self::MappedBytes(segments) => read_mapped_byte_range(reader, segments, offset, buf),
            Self::Memory(data) => {
                let start = usize::try_from(offset).ok()?;
                let end = start.checked_add(buf.len())?;
                buf.copy_from_slice(data.get(start..end)?);
                Some(())
            }
            Self::WimResource {
                wim,
                metadata,
                resource,
            } => wim::read_resource_range_with(
                metadata,
                wim.size,
                resource,
                offset,
                buf,
                |wim_offset, wim_buf| {
                    wim.read_range(reader, wim_offset, wim_buf)
                        .ok_or(wim::WimReadError::ResourceOutOfBounds)
                },
            )
            .ok(),
        }
    }
}

impl WimbootRuntimeFile {
    pub(super) fn from_iso(iso: &IsoFile, callback_path: &str) -> uefi::Result<Self> {
        if iso.extents.is_empty() || iso.block_size == 0 {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size: iso.size,
            storage: WimbootRuntimeFileStorage::Disk {
                block_size: iso.block_size,
                extents: iso.extents.clone(),
            },
        })
    }

    pub(super) fn from_source_file(
        file: &SourceVolumeFileMetadata,
        callback_path: &str,
    ) -> uefi::Result<Self> {
        if file.extents.is_empty() || file.block_size == 0 {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size: file.size,
            storage: WimbootRuntimeFileStorage::Disk {
                block_size: file.block_size,
                extents: file.extents.clone(),
            },
        })
    }

    pub(super) fn from_mapped_segments(
        callback_path: &str,
        size: u64,
        segments: Vec<WimbootMappedSegment>,
    ) -> uefi::Result<Self> {
        if size != 0 && segments.is_empty() {
            return Err(Status::UNSUPPORTED.into());
        }

        Ok(Self {
            callback_path: String::from(callback_path),
            size,
            storage: WimbootRuntimeFileStorage::MappedBytes(segments),
        })
    }

    pub(super) fn from_memory(callback_path: &str, data: Vec<u8>) -> Self {
        Self {
            callback_path: String::from(callback_path),
            size: data.len() as u64,
            storage: WimbootRuntimeFileStorage::Memory(data),
        }
    }

    pub(super) fn from_wim_resource(
        callback_path: &str,
        wim: &WimbootRuntimeFile,
        metadata: wim::WimMetadata,
        resource: wim::WimResourceHeader,
    ) -> Self {
        Self {
            callback_path: String::from(callback_path),
            size: resource.uncompressed_size,
            storage: WimbootRuntimeFileStorage::WimResource {
                wim: Box::new(wim.clone()),
                metadata,
                resource,
            },
        }
    }

    pub(super) fn size_i32(&self) -> Option<i32> {
        i32::try_from(self.size).ok()
    }

    pub(super) fn read_range(
        &self,
        reader: &SourceVolumeReader,
        offset: u64,
        buf: &mut [u8],
    ) -> Option<()> {
        let end = offset.checked_add(buf.len() as u64)?;
        if end > self.size {
            return None;
        }

        self.storage.read_range(reader, offset, buf)
    }
}

fn read_extent_range(
    reader: &SourceVolumeReader,
    block_size: u32,
    extents: &[IsoExtent],
    offset: u64,
    buf: &mut [u8],
) -> Option<()> {
    let end = offset.checked_add(buf.len() as u64)?;
    let block_size_u64 = u64::from(block_size);
    let mut cursor = offset;
    let mut copied = 0usize;

    while copied < buf.len() {
        let extent = extents.iter().find(|extent| {
            let Some(extent_start) = extent.virtual_block_start.checked_mul(block_size_u64) else {
                return false;
            };
            let Some(extent_bytes) = extent.block_count.checked_mul(block_size_u64) else {
                return false;
            };
            let Some(extent_end) = extent_start.checked_add(extent_bytes) else {
                return false;
            };
            cursor >= extent_start && cursor < extent_end
        })?;

        let extent_start = extent.virtual_block_start.checked_mul(block_size_u64)?;
        let extent_bytes = extent.block_count.checked_mul(block_size_u64)?;
        let extent_end = extent_start.checked_add(extent_bytes)?;
        let read_end = end.min(extent_end);
        let read_len = usize::try_from(read_end.checked_sub(cursor)?).ok()?;
        let physical_byte = extent
            .physical_lba
            .checked_mul(block_size_u64)?
            .checked_add(cursor.checked_sub(extent_start)?)?;

        read_physical_bytes(
            reader,
            block_size,
            physical_byte,
            &mut buf[copied..copied + read_len],
        )?;

        cursor = read_end;
        copied += read_len;
    }

    Some(())
}

fn read_mapped_byte_range(
    reader: &SourceVolumeReader,
    segments: &[WimbootMappedSegment],
    offset: u64,
    buf: &mut [u8],
) -> Option<()> {
    let end = offset.checked_add(buf.len() as u64)?;
    let mut cursor = offset;
    let mut copied = 0usize;

    while copied < buf.len() {
        let segment = segments.iter().find(|segment| {
            let Some(segment_end) = segment.virtual_offset.checked_add(segment.byte_count) else {
                return false;
            };
            cursor >= segment.virtual_offset && cursor < segment_end
        })?;

        let segment_end = segment.virtual_offset.checked_add(segment.byte_count)?;
        let read_end = end.min(segment_end);
        let read_len = usize::try_from(read_end.checked_sub(cursor)?).ok()?;
        let physical_byte = segment
            .physical_offset
            .checked_add(cursor.checked_sub(segment.virtual_offset)?)?;

        read_physical_bytes(
            reader,
            reader.block_size(),
            physical_byte,
            &mut buf[copied..copied + read_len],
        )?;

        cursor = read_end;
        copied += read_len;
    }

    Some(())
}

fn read_physical_bytes(
    reader: &SourceVolumeReader,
    block_size: u32,
    physical_byte_start: u64,
    buf: &mut [u8],
) -> Option<()> {
    let block_size = usize::try_from(block_size).ok()?;
    if block_size == 0 {
        return None;
    }

    let mut scratch = Vec::new();
    scratch.try_reserve_exact(block_size).ok()?;
    scratch.resize(block_size, 0);

    let mut physical_byte = physical_byte_start;
    let mut copied = 0usize;

    while copied < buf.len() {
        let physical_lba = physical_byte / block_size as u64;
        let in_block_offset = usize::try_from(physical_byte % block_size as u64).ok()?;
        let copy_size = (block_size - in_block_offset).min(buf.len() - copied);

        PhysicalReader::read_blocks(reader, physical_lba, &mut scratch).ok()?;
        buf[copied..copied + copy_size]
            .copy_from_slice(&scratch[in_block_offset..in_block_offset + copy_size]);

        physical_byte = physical_byte.checked_add(copy_size as u64)?;
        copied += copy_size;
    }

    Some(())
}
