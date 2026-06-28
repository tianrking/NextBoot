//! Ventoy-compatible Linux supplemental initrd builder.
//!
//! Ventoy boots Linux by prepending/appending a small `newc` cpio payload with
//! files under `ventoy/`. The early userspace hooks read those files to locate
//! the selected image, optional injection archives, DUD files, and install
//! templates. This module keeps that wire format testable outside the firmware
//! entry point.

use crate::ventoy::{VentoyExtent, VENTOY_OS_PARAM_SIZE};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

mod cpio;

pub const VENTOY_IMAGE_MAP_PATH: &str = "ventoy/ventoy_image_map";
pub const VENTOY_PERSISTENT_MAP_PATH: &str = "ventoy/ventoy_persistent_map";
pub const VENTOY_AUTOINSTALL_PATH: &str = "ventoy/autoinstall";
pub const VENTOY_INJECTION_PATH: &str = "ventoy/ventoy_injection";
pub const VENTOY_OS_PARAM_PATH: &str = "ventoy/ventoy_os_param";

const VENTOY_CHUNK_SIZE: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyLinuxInitrdError {
    InvalidArchive,
    InvalidSectorSize,
    UnalignedExtent,
    ValueOutOfRange,
    NameTooLong,
    FileTooLarge,
    OutputReserveFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyImageMapChunk {
    pub img_start_sector: u32,
    pub img_end_sector: u32,
    pub disk_start_sector: u64,
    pub disk_end_sector: u64,
}

pub struct VentoyDudFile<'a> {
    pub source_path: &'a str,
    pub data: &'a [u8],
}

pub struct VentoyLinuxInitrdInput<'a> {
    pub base_archives: &'a [&'a [u8]],
    pub original_initrd: Option<&'a [u8]>,
    pub image_map: &'a [VentoyImageMapChunk],
    pub os_param: &'a [u8; VENTOY_OS_PARAM_SIZE],
    pub auto_install: Option<&'a [u8]>,
    pub persistent_map: Option<&'a [VentoyImageMapChunk]>,
    pub injection_archive: Option<&'a [u8]>,
    pub dud_files: &'a [VentoyDudFile<'a>],
}

pub fn build_ventoy_linux_initrd(
    input: &VentoyLinuxInitrdInput<'_>,
) -> Result<Vec<u8>, VentoyLinuxInitrdError> {
    let mut builder = cpio::NewcArchiveBuilder::new();

    for archive in input.base_archives {
        builder.append_archive_without_trailer(archive)?;
    }

    if let Some(data) = input.original_initrd {
        builder.add_file("initrd000", data)?;
    }

    builder.add_file(VENTOY_IMAGE_MAP_PATH, &encode_image_map(input.image_map)?)?;

    if let Some(data) = input.auto_install {
        builder.add_file(VENTOY_AUTOINSTALL_PATH, data)?;
    }

    if let Some(chunks) = input.persistent_map {
        builder.add_file(VENTOY_PERSISTENT_MAP_PATH, &encode_image_map(chunks)?)?;
    }

    if let Some(data) = input.injection_archive {
        builder.add_file(VENTOY_INJECTION_PATH, data)?;
    }

    for (index, dud) in input.dud_files.iter().enumerate() {
        let name = dud_entry_name(index, dud.source_path)?;
        builder.add_file(&name, dud.data)?;
    }

    builder.add_file(VENTOY_OS_PARAM_PATH, input.os_param)?;
    builder.finish()
}

pub fn build_image_map_chunks(
    extents: &[VentoyExtent],
    source_block_size: u32,
    image_sector_size: u32,
) -> Result<Vec<VentoyImageMapChunk>, VentoyLinuxInitrdError> {
    if source_block_size == 0 || image_sector_size == 0 {
        return Err(VentoyLinuxInitrdError::InvalidSectorSize);
    }

    let source_block_size = u64::from(source_block_size);
    let image_sector_size = u64::from(image_sector_size);
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(extents.len())
        .map_err(|_| VentoyLinuxInitrdError::OutputReserveFailed)?;

    for extent in extents {
        if extent.block_count == 0 {
            continue;
        }

        let image_start_bytes = extent
            .virtual_block_start
            .checked_mul(source_block_size)
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        let image_bytes = extent
            .block_count
            .checked_mul(source_block_size)
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        if image_start_bytes % image_sector_size != 0 || image_bytes % image_sector_size != 0 {
            return Err(VentoyLinuxInitrdError::UnalignedExtent);
        }

        let image_start_sector = image_start_bytes / image_sector_size;
        let image_sector_count = image_bytes / image_sector_size;
        let image_end_sector = image_start_sector
            .checked_add(image_sector_count)
            .and_then(|value| value.checked_sub(1))
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
        let disk_end_sector = extent
            .physical_lba
            .checked_add(extent.block_count)
            .and_then(|value| value.checked_sub(1))
            .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;

        chunks.push(VentoyImageMapChunk {
            img_start_sector: u32::try_from(image_start_sector)
                .map_err(|_| VentoyLinuxInitrdError::ValueOutOfRange)?,
            img_end_sector: u32::try_from(image_end_sector)
                .map_err(|_| VentoyLinuxInitrdError::ValueOutOfRange)?,
            disk_start_sector: extent.physical_lba,
            disk_end_sector,
        });
    }

    Ok(chunks)
}

pub fn encode_image_map(chunks: &[VentoyImageMapChunk]) -> Result<Vec<u8>, VentoyLinuxInitrdError> {
    let total = chunks
        .len()
        .checked_mul(VENTOY_CHUNK_SIZE)
        .ok_or(VentoyLinuxInitrdError::ValueOutOfRange)?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| VentoyLinuxInitrdError::OutputReserveFailed)?;

    for chunk in chunks {
        push_u32(&mut out, chunk.img_start_sector);
        push_u32(&mut out, chunk.img_end_sector);
        push_u64(&mut out, chunk.disk_start_sector);
        push_u64(&mut out, chunk.disk_end_sector);
    }

    Ok(out)
}

fn dud_entry_name(index: usize, source_path: &str) -> Result<String, VentoyLinuxInitrdError> {
    let extension = path_extension(source_path).unwrap_or(".iso");
    let mut name = String::from("ventoy/ventoy_dud");
    name.push_str(index.to_string().as_str());
    name.push_str(extension);
    Ok(name)
}

fn path_extension(path: &str) -> Option<&str> {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let dot = filename.rfind('.')?;
    Some(&filename[dot..])
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::cpio::{
        align_up, parse_hex_field, NewcArchiveBuilder, CPIO_FINAL_ALIGNMENT, CPIO_HEADER_LEN,
        CPIO_TRAILER,
    };
    use super::*;

    #[test]
    fn builds_ventoy_supplemental_archive_entries() {
        let os_param = [0x5a; VENTOY_OS_PARAM_SIZE];
        let image_map = [VentoyImageMapChunk {
            img_start_sector: 0,
            img_end_sector: 3,
            disk_start_sector: 100,
            disk_end_sector: 115,
        }];
        let dud = VentoyDudFile {
            source_path: "/dud/dd.iso",
            data: b"dud",
        };
        let input = VentoyLinuxInitrdInput {
            base_archives: &[],
            original_initrd: None,
            image_map: &image_map,
            os_param: &os_param,
            auto_install: Some(b"answer"),
            persistent_map: None,
            injection_archive: Some(b"tar"),
            dud_files: &[dud],
        };

        let archive = build_ventoy_linux_initrd(&input).expect("archive");
        let entries = list_entries(&archive).expect("entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.0.as_str())
                .collect::<Vec<_>>(),
            [
                VENTOY_IMAGE_MAP_PATH,
                VENTOY_AUTOINSTALL_PATH,
                VENTOY_INJECTION_PATH,
                "ventoy/ventoy_dud0.iso",
                VENTOY_OS_PARAM_PATH,
                CPIO_TRAILER,
            ]
        );
        assert_eq!(
            entry_data(&archive, VENTOY_IMAGE_MAP_PATH).expect("image map"),
            encode_image_map(&image_map).expect("encoded map")
        );
        assert_eq!(
            entry_data(&archive, VENTOY_OS_PARAM_PATH).expect("os param"),
            os_param
        );
        assert_eq!(archive.len() % CPIO_FINAL_ALIGNMENT, 0);
    }

    #[test]
    fn appends_after_base_archive_without_duplicate_trailer() {
        let os_param = [0; VENTOY_OS_PARAM_SIZE];
        let image_map = [VentoyImageMapChunk {
            img_start_sector: 0,
            img_end_sector: 0,
            disk_start_sector: 1,
            disk_end_sector: 4,
        }];
        let mut base = NewcArchiveBuilder::new();
        base.add_file("init", b"base").expect("base file");
        let base = base.finish().expect("base archive");
        let input = VentoyLinuxInitrdInput {
            base_archives: &[&base],
            original_initrd: Some(b"initrd"),
            image_map: &image_map,
            os_param: &os_param,
            auto_install: None,
            persistent_map: None,
            injection_archive: None,
            dud_files: &[],
        };

        let archive = build_ventoy_linux_initrd(&input).expect("archive");
        let entries = list_entries(&archive).expect("entries");

        assert_eq!(entries[0].0, "init");
        assert!(entries.iter().any(|entry| entry.0 == "initrd000"));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.0 == CPIO_TRAILER)
                .count(),
            1
        );
    }

    #[test]
    fn writes_persistent_map_entry_when_configured() {
        let os_param = [0; VENTOY_OS_PARAM_SIZE];
        let image_map = [VentoyImageMapChunk {
            img_start_sector: 0,
            img_end_sector: 0,
            disk_start_sector: 1,
            disk_end_sector: 1,
        }];
        let persistent_map = [VentoyImageMapChunk {
            img_start_sector: 0,
            img_end_sector: 2047,
            disk_start_sector: 4096,
            disk_end_sector: 6143,
        }];
        let input = VentoyLinuxInitrdInput {
            base_archives: &[],
            original_initrd: None,
            image_map: &image_map,
            os_param: &os_param,
            auto_install: None,
            persistent_map: Some(&persistent_map),
            injection_archive: None,
            dud_files: &[],
        };

        let archive = build_ventoy_linux_initrd(&input).expect("archive");

        assert_eq!(
            entry_data(&archive, VENTOY_PERSISTENT_MAP_PATH).expect("persistent map"),
            encode_image_map(&persistent_map).expect("encoded persistent map")
        );
    }

    #[test]
    fn maps_extents_to_ventoy_chunks() {
        let extents = [
            VentoyExtent {
                virtual_block_start: 0,
                physical_lba: 100,
                block_count: 8,
            },
            VentoyExtent {
                virtual_block_start: 8,
                physical_lba: 240,
                block_count: 4,
            },
        ];

        let chunks = build_image_map_chunks(&extents, 512, 2048).expect("chunks");

        assert_eq!(
            chunks,
            [
                VentoyImageMapChunk {
                    img_start_sector: 0,
                    img_end_sector: 1,
                    disk_start_sector: 100,
                    disk_end_sector: 107,
                },
                VentoyImageMapChunk {
                    img_start_sector: 2,
                    img_end_sector: 2,
                    disk_start_sector: 240,
                    disk_end_sector: 243,
                },
            ]
        );
    }

    fn list_entries(archive: &[u8]) -> Result<Vec<(String, usize, usize)>, VentoyLinuxInitrdError> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        loop {
            let header = archive
                .get(offset..offset + CPIO_HEADER_LEN)
                .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
            let file_size = parse_hex_field(header, 54)? as usize;
            let name_size = parse_hex_field(header, 94)? as usize;
            let name_start = offset + CPIO_HEADER_LEN;
            let name_end = name_start + name_size;
            let name_bytes = archive
                .get(name_start..name_end)
                .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
            let name = core::str::from_utf8(&name_bytes[..name_size - 1])
                .map_err(|_| VentoyLinuxInitrdError::InvalidArchive)?
                .to_string();
            let data_start = align_up(name_end, 4).ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
            out.push((name.clone(), data_start, file_size));
            offset = align_up(data_start + file_size, 4)
                .ok_or(VentoyLinuxInitrdError::InvalidArchive)?;
            if name == CPIO_TRAILER {
                break;
            }
        }
        Ok(out)
    }

    fn entry_data(archive: &[u8], name: &str) -> Option<Vec<u8>> {
        list_entries(archive)
            .ok()?
            .into_iter()
            .find(|entry| entry.0 == name)
            .and_then(|(_, start, size)| archive.get(start..start + size).map(|data| data.to_vec()))
    }
}
