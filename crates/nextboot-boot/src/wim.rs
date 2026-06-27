//! WIM/ESD metadata helpers.
//!
//! Ventoy's WIMBOOT path first inspects the WIM header for the `MSWIM\0\0`
//! signature, boot index, compression flags, and resource descriptors.  This
//! module keeps that parsing independent from UEFI I/O so scanner and future
//! wimboot chain-loading code can share the same validation.

#![allow(dead_code)]

extern crate alloc;

mod format;
#[path = "lzx.rs"]
mod lzx;
mod path;
mod resource;
mod xpress;

#[allow(unused_imports)]
pub use format::{
    compression_from_flags, lookup_resource_for_hash, metadata_resource_count,
    metadata_resource_for_image, parse_lookup_entry, parse_wim_metadata,
};
#[allow(unused_imports)]
pub use path::{file_resource_for_path, find_path_entry};
#[allow(unused_imports)]
pub use resource::{read_resource_range, read_resource_range_with};
pub use xpress::decompress_xpress;

pub const WIM_HEADER_SIZE: usize = 208;
pub const WIM_SIGNATURE: &[u8; 8] = b"MSWIM\0\0\0";
pub const WIM_RESHDR_ZLEN_MASK: u64 = 0x00ff_ffff_ffff_ffff;
pub const WIM_RESHDR_METADATA: u8 = 0x02;
pub const WIM_RESHDR_COMPRESSED: u8 = 0x04;
pub const WIM_RESHDR_PACKED_STREAMS: u8 = 0x10;
pub const WIM_HDR_XPRESS: u32 = 0x0002_0000;
pub const WIM_HDR_LZX: u32 = 0x0004_0000;
pub const WIM_HDR_LZMS: u32 = 0x0008_0000;
pub const WIM_HASH_SIZE: usize = 20;
pub const WIM_LOOKUP_ENTRY_SIZE: usize = 50;
pub const WIM_DIRECTORY_ENTRY_FIXED_SIZE: usize = 102;
pub const WIM_SECURITY_HEADER_SIZE: usize = 8;
pub const WIM_ATTR_NORMAL: u32 = 0x0000_0080;
pub const WIM_NO_SECURITY: u32 = 0xffff_ffff;
pub const WIM_MAX_U32_RESOURCE_SIZE: u64 = 0xffff_ffff;
pub const XPRESS_CODE_COUNT: usize = 512;
pub const XPRESS_LENGTH_TABLE_SIZE: usize = XPRESS_CODE_COUNT / 2;
pub const XPRESS_END_MARKER: u16 = 256;
pub const XPRESS_BLOCK_SIZE: usize = 64 * 1024;
pub const HUFFMAN_BITS: usize = 16;

const HEADER_LEN_OFFSET: usize = 8;
const VERSION_OFFSET: usize = 12;
const FLAGS_OFFSET: usize = 16;
const CHUNK_LEN_OFFSET: usize = 20;
const PART_OFFSET: usize = 40;
const PARTS_OFFSET: usize = 42;
const IMAGE_COUNT_OFFSET: usize = 44;
const LOOKUP_RESOURCE_OFFSET: usize = 48;
const XML_RESOURCE_OFFSET: usize = 72;
const BOOT_RESOURCE_OFFSET: usize = 96;
const BOOT_INDEX_OFFSET: usize = 120;
const INTEGRITY_RESOURCE_OFFSET: usize = 124;
const RESOURCE_HEADER_SIZE: usize = 24;
const LOOKUP_ENTRY_PART_OFFSET: usize = RESOURCE_HEADER_SIZE;
const LOOKUP_ENTRY_REFCNT_OFFSET: usize = LOOKUP_ENTRY_PART_OFFSET + 2;
const LOOKUP_ENTRY_HASH_OFFSET: usize = LOOKUP_ENTRY_REFCNT_OFFSET + 4;
const DIRECTORY_ENTRY_ATTRIBUTES_OFFSET: usize = 8;
const DIRECTORY_ENTRY_SECURITY_OFFSET: usize = 12;
const DIRECTORY_ENTRY_SUBDIR_OFFSET: usize = 16;
const DIRECTORY_ENTRY_HASH_OFFSET: usize = 64;
const DIRECTORY_ENTRY_STREAMS_OFFSET: usize = 96;
const DIRECTORY_ENTRY_SHORT_NAME_LEN_OFFSET: usize = 98;
const DIRECTORY_ENTRY_NAME_LEN_OFFSET: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimCompression {
    None,
    Xpress,
    Lzx,
    Lzms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimResourceHeader {
    pub compressed_size: u64,
    pub flags: u8,
    pub offset: u64,
    pub uncompressed_size: u64,
}

impl WimResourceHeader {
    pub fn is_empty(&self) -> bool {
        self.compressed_size == 0 && self.offset == 0 && self.uncompressed_size == 0
    }

    pub fn is_metadata(&self) -> bool {
        self.flags & WIM_RESHDR_METADATA != 0
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & WIM_RESHDR_COMPRESSED != 0
    }

    pub fn uses_packed_streams(&self) -> bool {
        self.flags & WIM_RESHDR_PACKED_STREAMS != 0
    }

    pub fn file_end(&self) -> Option<u64> {
        self.offset.checked_add(self.compressed_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimLookupEntry {
    pub resource: WimResourceHeader,
    pub part: u16,
    pub ref_count: u32,
    pub hash: [u8; WIM_HASH_SIZE],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimDirectoryEntry {
    pub len: u64,
    pub attributes: u32,
    pub security: u32,
    pub subdir: u64,
    pub hash: [u8; WIM_HASH_SIZE],
    pub streams: u16,
    pub short_name_len: u16,
    pub name_len: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimDirectoryEntryLocation {
    pub offset: usize,
    pub entry: WimDirectoryEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimPathError {
    EmptyPath,
    NonAsciiPath,
    MalformedMetadata,
    NotFound,
    ResourceNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WimReadError {
    InvalidChunkLength,
    InvalidRange,
    InvalidChunkTable,
    ResourceOutOfBounds,
    OutputReserveFailed,
    XpressDecodeFailed(XpressDecodeError),
    LzxDecodeFailed(lzx::LzxDecodeError),
    UnsupportedCompressedChunk {
        chunk_index: u64,
        compressed_size: u64,
        uncompressed_size: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XpressDecodeError {
    InputTooShort,
    InvalidHuffmanLength,
    IncompleteHuffmanAlphabet,
    InvalidHuffmanCode,
    OutputOverflow,
    InvalidMatchOffset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WimMetadata {
    pub header_len: u32,
    pub version: u32,
    pub flags: u32,
    pub compression: WimCompression,
    pub chunk_len: u32,
    pub part: u16,
    pub parts: u16,
    pub image_count: u32,
    pub boot_index: u32,
    pub lookup: WimResourceHeader,
    pub xml: WimResourceHeader,
    pub boot: WimResourceHeader,
    pub integrity: WimResourceHeader,
}

impl WimMetadata {
    pub fn is_bootable(&self) -> bool {
        self.boot_index != 0
    }

    pub fn boot_index_in_range(&self) -> bool {
        self.boot_index != 0 && self.boot_index <= self.image_count
    }

    pub fn is_wimboot_supported(&self) -> bool {
        self.boot_index_in_range() && self.compression != WimCompression::Lzms
    }
}

fn read_le_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_le_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[cfg(test)]
mod tests;
