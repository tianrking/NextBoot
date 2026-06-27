use super::VentoyLinuxInitrdError;
use alloc::vec::Vec;

const CPIO_MAGIC_NEWC: &[u8; 6] = b"070701";
pub(super) const CPIO_HEADER_LEN: usize = 110;
const CPIO_FILE_MODE: u32 = 0o100777;
pub(super) const CPIO_TRAILER: &str = "TRAILER!!!";
pub(super) const CPIO_FINAL_ALIGNMENT: usize = 512;

pub(super) struct NewcArchiveBuilder {
    data: Vec<u8>,
    next_ino: u32,
}

impl NewcArchiveBuilder {
    pub(super) fn new() -> Self {
        Self {
            data: Vec::new(),
            next_ino: 0xffff_fff0,
        }
    }

    pub(super) fn append_archive_without_trailer(
        &mut self,
        archive: &[u8],
    ) -> Result<(), VentoyLinuxInitrdError> {
        let end = archive_payload_end(archive)?;
        self.data
            .try_reserve_exact(end)
            .map_err(|_| VentoyLinuxInitrdError::OutputReserveFailed)?;
        self.data.extend_from_slice(&archive[..end]);
        Ok(())
    }

    pub(super) fn add_file(
        &mut self,
        name: &str,
        contents: &[u8],
    ) -> Result<(), VentoyLinuxInitrdError> {
        self.add_entry(name, CPIO_FILE_MODE, contents)
    }

    pub(super) fn finish(mut self) -> Result<Vec<u8>, VentoyLinuxInitrdError> {
        self.add_entry(CPIO_TRAILER, 0, &[])?;
        align_vec(&mut self.data, CPIO_FINAL_ALIGNMENT);
        Ok(self.data)
    }

    fn add_entry(
        &mut self,
        name: &str,
        mode: u32,
        contents: &[u8],
    ) -> Result<(), VentoyLinuxInitrdError> {
        if name.as_bytes().contains(&0) {
            return Err(VentoyLinuxInitrdError::NameTooLong);
        }
        let name_size = name
            .len()
            .checked_add(1)
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        let file_size =
            u32::try_from(contents.len()).map_err(|_| VentoyLinuxInitrdError::FileTooLarge)?;
        let header_and_name = CPIO_HEADER_LEN
            .checked_add(name_size)
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        let data_start =
            align_up(header_and_name, 4).ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        let total = data_start
            .checked_add(
                align_up(contents.len(), 4).ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?,
            )
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;

        self.data
            .try_reserve_exact(total)
            .map_err(|_| VentoyLinuxInitrdError::OutputReserveFailed)?;

        self.data.extend_from_slice(CPIO_MAGIC_NEWC);
        push_hex_field(&mut self.data, self.next_ino);
        self.next_ino = self.next_ino.wrapping_sub(1);
        push_hex_field(&mut self.data, mode);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, 1);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, file_size);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, 0);
        push_hex_field(&mut self.data, 0);
        push_hex_field(
            &mut self.data,
            u32::try_from(name_size).map_err(|_| VentoyLinuxInitrdError::NameTooLong)?,
        );
        push_hex_field(&mut self.data, 0);
        self.data.extend_from_slice(name.as_bytes());
        self.data.push(0);
        align_vec(&mut self.data, 4);
        self.data.extend_from_slice(contents);
        align_vec(&mut self.data, 4);

        Ok(())
    }
}

fn archive_payload_end(archive: &[u8]) -> Result<usize, VentoyLinuxInitrdError> {
    if archive.is_empty() {
        return Ok(0);
    }

    let mut offset = 0usize;
    while offset < archive.len() {
        let entry = parse_entry(archive, offset)?;
        if entry.name == CPIO_TRAILER {
            return Ok(offset);
        }
        offset = entry.next_offset;
    }

    Err(VentoyLinuxInitrdError::InvalidArchive)
}

fn parse_entry(archive: &[u8], offset: usize) -> Result<ParsedEntry<'_>, VentoyLinuxInitrdError> {
    let header = archive
        .get(offset..offset + CPIO_HEADER_LEN)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    if header.get(..CPIO_MAGIC_NEWC.len()) != Some(CPIO_MAGIC_NEWC) {
        return Err(VentoyLinuxInitrdError::InvalidArchive);
    }

    let file_size = parse_hex_field(header, 54)? as usize;
    let name_size = parse_hex_field(header, 94)? as usize;
    if name_size == 0 {
        return Err(VentoyLinuxInitrdError::InvalidArchive);
    }

    let name_start = offset
        .checked_add(CPIO_HEADER_LEN)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    let name_end = name_start
        .checked_add(name_size)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    let name_bytes = archive
        .get(name_start..name_end)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    if name_bytes.last().copied() != Some(0) {
        return Err(VentoyLinuxInitrdError::InvalidArchive);
    }
    let name = core::str::from_utf8(&name_bytes[..name_bytes.len() - 1])
        .map_err(|_| VentoyLinuxInitrdError::InvalidArchive)?;

    let data_start = align_up(name_end, 4).ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    let data_end = data_start
        .checked_add(file_size)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    let next_offset = align_up(data_end, 4).ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    if next_offset > archive.len() {
        return Err(VentoyLinuxInitrdError::InvalidArchive);
    }

    Ok(ParsedEntry { name, next_offset })
}

struct ParsedEntry<'a> {
    name: &'a str,
    next_offset: usize,
}

pub(super) fn parse_hex_field(header: &[u8], offset: usize) -> Result<u32, VentoyLinuxInitrdError> {
    let field = header
        .get(offset..offset + 8)
        .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    let mut value = 0u32;
    for byte in field {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a' + 10),
            b'A'..=b'F' => u32::from(byte - b'A' + 10),
            _ => return Err(VentoyLinuxInitrdError::InvalidArchive),
        };
        value = value
            .checked_mul(16)
            .and_then(|base| base.checked_add(digit))
            .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
    }
    Ok(value)
}

fn push_hex_field(out: &mut Vec<u8>, value: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..8).rev() {
        out.push(HEX[((value >> (shift * 4)) & 0xf) as usize]);
    }
}

pub(super) fn align_up(value: usize, align: usize) -> Option<usize> {
    if align == 0 {
        return None;
    }
    value.checked_add(align - 1).map(|sum| sum / align * align)
}

fn align_vec(data: &mut Vec<u8>, align: usize) {
    if let Some(next) = align_up(data.len(), align) {
        data.resize(next, 0);
    }
}
