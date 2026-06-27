//! WIM/ESD metadata helpers.
//!
//! Ventoy's WIMBOOT path first inspects the WIM header for the `MSWIM\0\0`
//! signature, boot index, compression flags, and resource descriptors.  This
//! module keeps that parsing independent from UEFI I/O so scanner and future
//! wimboot chain-loading code can share the same validation.

#![allow(dead_code)]

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

pub fn parse_lookup_entry(data: &[u8]) -> Option<WimLookupEntry> {
    if data.len() < WIM_LOOKUP_ENTRY_SIZE {
        return None;
    }

    let mut hash = [0u8; WIM_HASH_SIZE];
    hash.copy_from_slice(data.get(LOOKUP_ENTRY_HASH_OFFSET..WIM_LOOKUP_ENTRY_SIZE)?);

    Some(WimLookupEntry {
        resource: parse_resource_header(data, 0)?,
        part: read_le_u16(data, LOOKUP_ENTRY_PART_OFFSET)?,
        ref_count: read_le_u32(data, LOOKUP_ENTRY_REFCNT_OFFSET)?,
        hash,
    })
}

pub fn metadata_resource_count(lookup_table: &[u8]) -> usize {
    lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
        .filter(|entry| entry.resource.is_metadata())
        .count()
}

pub fn metadata_resource_for_image(
    metadata: &WimMetadata,
    lookup_table: &[u8],
    image_index: u32,
) -> Option<WimResourceHeader> {
    if image_index == 0 {
        return Some(metadata.boot);
    }

    let mut found = 0u32;
    for entry in lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
    {
        if entry.resource.is_metadata() {
            found = found.checked_add(1)?;
            if found == image_index {
                return Some(entry.resource);
            }
        }
    }

    None
}

pub fn lookup_resource_for_hash(
    lookup_table: &[u8],
    hash: &[u8; WIM_HASH_SIZE],
) -> Option<WimResourceHeader> {
    lookup_table
        .chunks_exact(WIM_LOOKUP_ENTRY_SIZE)
        .filter_map(parse_lookup_entry)
        .find(|entry| &entry.hash == hash)
        .map(|entry| entry.resource)
}

pub fn find_path_entry(
    metadata: &[u8],
    path: &str,
) -> Result<WimDirectoryEntryLocation, WimPathError> {
    if path.bytes().any(|byte| byte >= 0x80) {
        return Err(WimPathError::NonAsciiPath);
    }

    let mut components = path
        .split(|ch| ch == '\\' || ch == '/')
        .filter(|component| !component.is_empty());
    let mut component = components.next().ok_or(WimPathError::EmptyPath)?;
    let mut dir_offset = root_directory_offset(metadata)?;

    loop {
        let location = find_child_entry(metadata, dir_offset, component)?;
        if let Some(next) = components.next() {
            if location.entry.subdir == 0 {
                return Err(WimPathError::NotFound);
            }
            dir_offset = usize::try_from(location.entry.subdir)
                .map_err(|_| WimPathError::MalformedMetadata)?;
            component = next;
        } else {
            return Ok(location);
        }
    }
}

pub fn file_resource_for_path(
    metadata: &[u8],
    lookup_table: &[u8],
    path: &str,
) -> Result<WimResourceHeader, WimPathError> {
    let location = find_path_entry(metadata, path)?;
    lookup_resource_for_hash(lookup_table, &location.entry.hash)
        .ok_or(WimPathError::ResourceNotFound)
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

fn root_directory_offset(metadata: &[u8]) -> Result<usize, WimPathError> {
    if metadata.len() < WIM_SECURITY_HEADER_SIZE {
        return Err(WimPathError::MalformedMetadata);
    }

    let security_len = read_le_u32(metadata, 0).ok_or(WimPathError::MalformedMetadata)? as usize;
    let offset = if security_len > 0 {
        align_up_8(security_len).ok_or(WimPathError::MalformedMetadata)?
    } else {
        WIM_SECURITY_HEADER_SIZE
    };

    if offset > metadata.len() {
        return Err(WimPathError::MalformedMetadata);
    }

    Ok(offset)
}

fn find_child_entry(
    metadata: &[u8],
    dir_offset: usize,
    name: &str,
) -> Result<WimDirectoryEntryLocation, WimPathError> {
    let mut offset = dir_offset;

    loop {
        let entry_len = read_le_u64(metadata, offset).ok_or(WimPathError::MalformedMetadata)?;
        if entry_len == 0 {
            return Err(WimPathError::NotFound);
        }

        let entry_len = usize::try_from(entry_len).map_err(|_| WimPathError::MalformedMetadata)?;
        let entry_end = offset
            .checked_add(entry_len)
            .ok_or(WimPathError::MalformedMetadata)?;
        if entry_len < WIM_DIRECTORY_ENTRY_FIXED_SIZE || entry_end > metadata.len() {
            return Err(WimPathError::MalformedMetadata);
        }

        let entry = parse_directory_entry(metadata, offset)?;
        let name_start = offset
            .checked_add(WIM_DIRECTORY_ENTRY_FIXED_SIZE)
            .ok_or(WimPathError::MalformedMetadata)?;
        let name_end = name_start
            .checked_add(usize::from(entry.name_len))
            .ok_or(WimPathError::MalformedMetadata)?;
        if name_end > entry_end {
            return Err(WimPathError::MalformedMetadata);
        }

        if utf16le_name_eq_ascii(&metadata[name_start..name_end], name)? {
            return Ok(WimDirectoryEntryLocation { offset, entry });
        }

        offset = entry_end;
    }
}

fn parse_directory_entry(
    metadata: &[u8],
    offset: usize,
) -> Result<WimDirectoryEntry, WimPathError> {
    let mut hash = [0u8; WIM_HASH_SIZE];
    let hash_end = offset
        .checked_add(DIRECTORY_ENTRY_HASH_OFFSET + WIM_HASH_SIZE)
        .ok_or(WimPathError::MalformedMetadata)?;
    hash.copy_from_slice(
        metadata
            .get(offset + DIRECTORY_ENTRY_HASH_OFFSET..hash_end)
            .ok_or(WimPathError::MalformedMetadata)?,
    );

    Ok(WimDirectoryEntry {
        len: read_le_u64(metadata, offset).ok_or(WimPathError::MalformedMetadata)?,
        attributes: read_le_u32(metadata, offset + DIRECTORY_ENTRY_ATTRIBUTES_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        security: read_le_u32(metadata, offset + DIRECTORY_ENTRY_SECURITY_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        subdir: read_le_u64(metadata, offset + DIRECTORY_ENTRY_SUBDIR_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        hash,
        streams: read_le_u16(metadata, offset + DIRECTORY_ENTRY_STREAMS_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        short_name_len: read_le_u16(metadata, offset + DIRECTORY_ENTRY_SHORT_NAME_LEN_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
        name_len: read_le_u16(metadata, offset + DIRECTORY_ENTRY_NAME_LEN_OFFSET)
            .ok_or(WimPathError::MalformedMetadata)?,
    })
}

fn utf16le_name_eq_ascii(name_bytes: &[u8], ascii: &str) -> Result<bool, WimPathError> {
    if ascii.bytes().any(|byte| byte >= 0x80) {
        return Err(WimPathError::NonAsciiPath);
    }
    if name_bytes.len() % 2 != 0 {
        return Err(WimPathError::MalformedMetadata);
    }

    let mut units = name_bytes.len() / 2;
    let expected = ascii.len();
    if units == expected + 1
        && read_le_u16(name_bytes, expected * 2).ok_or(WimPathError::MalformedMetadata)? == 0
    {
        units -= 1;
    }
    if units != expected {
        return Ok(false);
    }

    for (index, expected) in ascii.bytes().enumerate() {
        let actual = read_le_u16(name_bytes, index * 2).ok_or(WimPathError::MalformedMetadata)?;
        if actual > 0x7f {
            return Ok(false);
        }
        if !ascii_eq_ignore_case(actual as u8, expected) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn ascii_eq_ignore_case(left: u8, right: u8) -> bool {
    left.eq_ignore_ascii_case(&right)
}

fn align_up_8(value: usize) -> Option<usize> {
    value.checked_add(7).map(|value| value & !7)
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

    #[test]
    fn selects_metadata_resources_from_lookup_table() {
        let header = make_wim_header(WIM_HDR_XPRESS, 2, 1);
        let metadata = parse_wim_metadata(&header).unwrap();
        let mut lookup = [0u8; WIM_LOOKUP_ENTRY_SIZE * 3];
        let first_hash = [0x11; WIM_HASH_SIZE];
        let file_hash = [0x22; WIM_HASH_SIZE];
        let second_hash = [0x33; WIM_HASH_SIZE];

        write_lookup_entry(
            &mut lookup,
            0,
            0x120,
            WIM_RESHDR_METADATA,
            0x10_000,
            0x400,
            1,
            1,
            &first_hash,
        );
        write_lookup_entry(
            &mut lookup,
            WIM_LOOKUP_ENTRY_SIZE,
            0x80,
            0,
            0x20_000,
            0x80,
            1,
            1,
            &file_hash,
        );
        write_lookup_entry(
            &mut lookup,
            WIM_LOOKUP_ENTRY_SIZE * 2,
            0x130,
            WIM_RESHDR_METADATA,
            0x30_000,
            0x500,
            1,
            1,
            &second_hash,
        );

        assert_eq!(metadata_resource_count(&lookup), 2);
        assert_eq!(
            metadata_resource_for_image(&metadata, &lookup, 0),
            Some(metadata.boot)
        );
        assert_eq!(
            metadata_resource_for_image(&metadata, &lookup, 1)
                .expect("first image metadata")
                .offset,
            0x10_000
        );
        assert_eq!(
            metadata_resource_for_image(&metadata, &lookup, 2)
                .expect("second image metadata")
                .offset,
            0x30_000
        );
        assert_eq!(metadata_resource_for_image(&metadata, &lookup, 3), None);
        assert_eq!(
            lookup_resource_for_hash(&lookup, &file_hash)
                .expect("file resource")
                .offset,
            0x20_000
        );
    }

    #[test]
    fn finds_file_resource_by_ascii_path() {
        let mut metadata = [0u8; 512];
        let root_offset = WIM_SECURITY_HEADER_SIZE;
        let windows_dir_offset = 160usize;
        let system32_dir_offset = 320usize;
        let file_hash = [0x7b; WIM_HASH_SIZE];
        let empty_hash = [0u8; WIM_HASH_SIZE];

        write_le_u32(&mut metadata, 0, 0);
        write_le_u32(&mut metadata, 4, 0);
        let windows_len = write_directory_entry(
            &mut metadata,
            root_offset,
            "Windows",
            windows_dir_offset,
            &empty_hash,
        );
        write_le_u64(&mut metadata, root_offset + windows_len, 0);

        let system32_len = write_directory_entry(
            &mut metadata,
            windows_dir_offset,
            "System32",
            system32_dir_offset,
            &empty_hash,
        );
        write_le_u64(&mut metadata, windows_dir_offset + system32_len, 0);

        let winpeshl_len = write_directory_entry(
            &mut metadata,
            system32_dir_offset,
            "winpeshl.exe",
            0,
            &file_hash,
        );
        write_le_u64(&mut metadata, system32_dir_offset + winpeshl_len, 0);

        let mut lookup = [0u8; WIM_LOOKUP_ENTRY_SIZE];
        write_lookup_entry(&mut lookup, 0, 0x200, 0, 0x44_000, 0x200, 1, 1, &file_hash);

        let entry =
            find_path_entry(&metadata, "/windows/system32/WINPESHL.EXE").expect("winpeshl entry");
        assert_eq!(entry.offset, system32_dir_offset);
        assert_eq!(entry.entry.hash, file_hash);
        assert_eq!(
            file_resource_for_path(&metadata, &lookup, "\\Windows\\System32\\winpeshl.exe")
                .expect("winpeshl resource")
                .offset,
            0x44_000
        );
        assert_eq!(
            find_path_entry(&metadata, "\\Windows\\System32\\missing.exe"),
            Err(WimPathError::NotFound)
        );
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

    #[allow(clippy::too_many_arguments)]
    fn write_lookup_entry(
        data: &mut [u8],
        offset: usize,
        compressed_size: u64,
        flags: u8,
        resource_offset: u64,
        uncompressed_size: u64,
        part: u16,
        ref_count: u32,
        hash: &[u8; WIM_HASH_SIZE],
    ) {
        write_resource_header(
            data,
            offset,
            compressed_size,
            flags,
            resource_offset,
            uncompressed_size,
        );
        write_le_u16(data, offset + LOOKUP_ENTRY_PART_OFFSET, part);
        write_le_u32(data, offset + LOOKUP_ENTRY_REFCNT_OFFSET, ref_count);
        data[offset + LOOKUP_ENTRY_HASH_OFFSET..offset + WIM_LOOKUP_ENTRY_SIZE]
            .copy_from_slice(hash);
    }

    fn write_directory_entry(
        data: &mut [u8],
        offset: usize,
        name: &str,
        subdir: usize,
        hash: &[u8; WIM_HASH_SIZE],
    ) -> usize {
        let name_len = name.len() * 2;
        let len = align_up_8(WIM_DIRECTORY_ENTRY_FIXED_SIZE + name_len).expect("entry length");
        write_le_u64(data, offset, len as u64);
        write_le_u32(
            data,
            offset + DIRECTORY_ENTRY_ATTRIBUTES_OFFSET,
            WIM_ATTR_NORMAL,
        );
        write_le_u32(
            data,
            offset + DIRECTORY_ENTRY_SECURITY_OFFSET,
            WIM_NO_SECURITY,
        );
        write_le_u64(data, offset + DIRECTORY_ENTRY_SUBDIR_OFFSET, subdir as u64);
        data[offset + DIRECTORY_ENTRY_HASH_OFFSET
            ..offset + DIRECTORY_ENTRY_HASH_OFFSET + WIM_HASH_SIZE]
            .copy_from_slice(hash);
        write_le_u16(
            data,
            offset + DIRECTORY_ENTRY_NAME_LEN_OFFSET,
            name_len as u16,
        );

        let mut name_offset = offset + WIM_DIRECTORY_ENTRY_FIXED_SIZE;
        for byte in name.bytes() {
            write_le_u16(data, name_offset, u16::from(byte));
            name_offset += 2;
        }

        len
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
