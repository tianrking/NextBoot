//! WIM/ESD metadata helpers.
//!
//! Ventoy's WIMBOOT path first inspects the WIM header for the `MSWIM\0\0`
//! signature, boot index, compression flags, and resource descriptors.  This
//! module keeps that parsing independent from UEFI I/O so scanner and future
//! wimboot chain-loading code can share the same validation.

pub const WIM_HEADER_SIZE: usize = 208;
pub const WIM_SIGNATURE: &[u8; 8] = b"MSWIM\0\0\0";
pub const WIM_RESHDR_ZLEN_MASK: u64 = 0x00ff_ffff_ffff_ffff;
pub const WIM_RESHDR_METADATA: u8 = 0x02;
pub const WIM_RESHDR_COMPRESSED: u8 = 0x04;
pub const WIM_RESHDR_PACKED_STREAMS: u8 = 0x10;
pub const WIM_HDR_XPRESS: u32 = 0x0002_0000;
pub const WIM_HDR_LZX: u32 = 0x0004_0000;
pub const WIM_HDR_LZMS: u32 = 0x0008_0000;

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

pub fn parse_wim_metadata(header: &[u8]) -> Option<WimMetadata> {
    if header.len() < WIM_HEADER_SIZE || header.get(0..8)? != WIM_SIGNATURE {
        return None;
    }

    let header_len = read_le_u32(header, HEADER_LEN_OFFSET)?;
    let version = read_le_u32(header, VERSION_OFFSET)?;
    let flags = read_le_u32(header, FLAGS_OFFSET)?;
    let chunk_len = read_le_u32(header, CHUNK_LEN_OFFSET)?;
    let part = read_le_u16(header, PART_OFFSET)?;
    let parts = read_le_u16(header, PARTS_OFFSET)?;
    let image_count = read_le_u32(header, IMAGE_COUNT_OFFSET)?;
    let boot_index = read_le_u32(header, BOOT_INDEX_OFFSET)?;

    if header_len < WIM_HEADER_SIZE as u32
        || version == 0
        || chunk_len == 0
        || part == 0
        || parts == 0
        || part > parts
        || image_count == 0
    {
        return None;
    }

    Some(WimMetadata {
        header_len,
        version,
        flags,
        compression: compression_from_flags(flags),
        chunk_len,
        part,
        parts,
        image_count,
        boot_index,
        lookup: parse_resource_header(header, LOOKUP_RESOURCE_OFFSET)?,
        xml: parse_resource_header(header, XML_RESOURCE_OFFSET)?,
        boot: parse_resource_header(header, BOOT_RESOURCE_OFFSET)?,
        integrity: parse_resource_header(header, INTEGRITY_RESOURCE_OFFSET)?,
    })
}

pub fn compression_from_flags(flags: u32) -> WimCompression {
    if flags & WIM_HDR_LZMS != 0 {
        WimCompression::Lzms
    } else if flags & WIM_HDR_LZX != 0 {
        WimCompression::Lzx
    } else if flags & WIM_HDR_XPRESS != 0 {
        WimCompression::Xpress
    } else {
        WimCompression::None
    }
}

fn parse_resource_header(data: &[u8], offset: usize) -> Option<WimResourceHeader> {
    let resource = data.get(offset..offset.checked_add(RESOURCE_HEADER_SIZE)?)?;
    let zlen_flags = read_le_u64(resource, 0)?;
    Some(WimResourceHeader {
        compressed_size: zlen_flags & WIM_RESHDR_ZLEN_MASK,
        flags: (zlen_flags >> 56) as u8,
        offset: read_le_u64(resource, 8)?,
        uncompressed_size: read_le_u64(resource, 16)?,
    })
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
mod tests {
    use super::*;

    #[test]
    fn parses_bootable_lzx_wim_header() {
        let header = make_wim_header(WIM_HDR_LZX, 2, 1);

        let metadata = parse_wim_metadata(&header).unwrap();
        assert_eq!(metadata.header_len, WIM_HEADER_SIZE as u32);
        assert_eq!(metadata.compression, WimCompression::Lzx);
        assert_eq!(metadata.image_count, 2);
        assert_eq!(metadata.boot_index, 1);
        assert!(metadata.is_bootable());
        assert!(metadata.boot_index_in_range());
        assert!(metadata.is_wimboot_supported());
        assert!(metadata.lookup.is_metadata());
        assert!(metadata.lookup.is_compressed());
        assert_eq!(metadata.lookup.file_end(), Some(0x5000));
    }

    #[test]
    fn marks_lzms_esd_as_not_supported_for_wimboot() {
        let header = make_wim_header(WIM_HDR_LZMS, 1, 1);
        let metadata = parse_wim_metadata(&header).unwrap();

        assert_eq!(metadata.compression, WimCompression::Lzms);
        assert!(metadata.is_bootable());
        assert!(!metadata.is_wimboot_supported());
    }

    #[test]
    fn treats_zero_boot_index_as_non_bootable() {
        let header = make_wim_header(WIM_HDR_XPRESS, 3, 0);
        let metadata = parse_wim_metadata(&header).unwrap();

        assert_eq!(metadata.compression, WimCompression::Xpress);
        assert!(!metadata.is_bootable());
        assert!(!metadata.boot_index_in_range());
        assert!(!metadata.is_wimboot_supported());
    }

    #[test]
    fn does_not_support_out_of_range_boot_index() {
        let header = make_wim_header(WIM_HDR_XPRESS, 1, 2);
        let metadata = parse_wim_metadata(&header).unwrap();

        assert!(metadata.is_bootable());
        assert!(!metadata.boot_index_in_range());
        assert!(!metadata.is_wimboot_supported());
    }

    #[test]
    fn rejects_bad_signature_and_invalid_header_values() {
        let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
        header[0] = b'X';
        assert!(parse_wim_metadata(&header).is_none());

        let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
        write_le_u32(&mut header, CHUNK_LEN_OFFSET, 0);
        assert!(parse_wim_metadata(&header).is_none());

        let mut header = make_wim_header(WIM_HDR_LZX, 1, 1);
        write_le_u16(&mut header, PART_OFFSET, 2);
        write_le_u16(&mut header, PARTS_OFFSET, 1);
        assert!(parse_wim_metadata(&header).is_none());
    }

    fn make_wim_header(flags: u32, image_count: u32, boot_index: u32) -> [u8; WIM_HEADER_SIZE] {
        let mut header = [0u8; WIM_HEADER_SIZE];
        header[0..8].copy_from_slice(WIM_SIGNATURE);
        write_le_u32(&mut header, HEADER_LEN_OFFSET, WIM_HEADER_SIZE as u32);
        write_le_u32(&mut header, VERSION_OFFSET, 0x0000_0d00);
        write_le_u32(&mut header, FLAGS_OFFSET, flags);
        write_le_u32(&mut header, CHUNK_LEN_OFFSET, 32 * 1024);
        header[24..40].copy_from_slice(&[0x42; 16]);
        write_le_u16(&mut header, PART_OFFSET, 1);
        write_le_u16(&mut header, PARTS_OFFSET, 1);
        write_le_u32(&mut header, IMAGE_COUNT_OFFSET, image_count);
        write_resource_header(
            &mut header,
            LOOKUP_RESOURCE_OFFSET,
            0x1000,
            WIM_RESHDR_METADATA | WIM_RESHDR_COMPRESSED,
            0x4000,
            0x2000,
        );
        write_resource_header(&mut header, XML_RESOURCE_OFFSET, 0x800, 0, 0x6000, 0x1200);
        write_resource_header(
            &mut header,
            BOOT_RESOURCE_OFFSET,
            0x200,
            WIM_RESHDR_METADATA,
            0x8000,
            0x400,
        );
        write_le_u32(&mut header, BOOT_INDEX_OFFSET, boot_index);
        write_resource_header(&mut header, INTEGRITY_RESOURCE_OFFSET, 0, 0, 0, 0);
        header
    }

    fn write_resource_header(
        data: &mut [u8],
        offset: usize,
        compressed_size: u64,
        flags: u8,
        resource_offset: u64,
        uncompressed_size: u64,
    ) {
        let zlen_flags = compressed_size | (u64::from(flags) << 56);
        write_le_u64(data, offset, zlen_flags);
        write_le_u64(data, offset + 8, resource_offset);
        write_le_u64(data, offset + 16, uncompressed_size);
    }

    fn write_le_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
