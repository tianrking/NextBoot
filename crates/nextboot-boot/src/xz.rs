//! Small XZ helpers for Ventoy-compatible compressed boot assets.

#[cfg(test)]
extern crate std;

use alloc::boxed::Box;
use alloc::vec::Vec;
use xz4rust::{XzDecoder, XzError, XzNextBlockResult, DICT_SIZE_MIN, DICT_SIZE_PROFILE_9};

const OUTPUT_CHUNK_SIZE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XzDecodeError {
    Decoder(XzError),
    OutputTooLarge,
    OutputReserveFailed,
    Stalled,
}

pub fn decompress_xz(input: &[u8], max_output_size: usize) -> Result<Vec<u8>, XzDecodeError> {
    let mut decoder = XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, DICT_SIZE_PROFILE_9);
    decompress_xz_with_decoder(input, max_output_size, &mut decoder)
}

fn decompress_xz_with_decoder(
    input: &[u8],
    max_output_size: usize,
    decoder: &mut Box<XzDecoder<'static>>,
) -> Result<Vec<u8>, XzDecodeError> {
    let mut output = Vec::new();
    let reserve_hint = input.len().saturating_mul(4).min(max_output_size);
    output
        .try_reserve(reserve_hint)
        .map_err(|_| XzDecodeError::OutputReserveFailed)?;

    let mut input_position = 0usize;
    let mut chunk = [0u8; OUTPUT_CHUNK_SIZE];

    loop {
        let result = decoder
            .decode(&input[input_position..], &mut chunk)
            .map_err(XzDecodeError::Decoder)?;
        let consumed = result.input_consumed();
        let produced = result.output_produced();
        input_position = input_position
            .checked_add(consumed)
            .ok_or(XzDecodeError::Stalled)?;

        if produced > 0 {
            let new_len = output
                .len()
                .checked_add(produced)
                .ok_or(XzDecodeError::OutputTooLarge)?;
            if new_len > max_output_size {
                return Err(XzDecodeError::OutputTooLarge);
            }
            output
                .try_reserve(produced)
                .map_err(|_| XzDecodeError::OutputReserveFailed)?;
            output.extend_from_slice(&chunk[..produced]);
        }

        match result {
            XzNextBlockResult::EndOfStream(_, _) => return Ok(output),
            XzNextBlockResult::NeedMoreData(_, _) if !result.made_progress() => {
                return Err(XzDecodeError::Stalled);
            }
            XzNextBlockResult::NeedMoreData(_, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VENTOY_ASSET_DIR: &str = "/Users/w0x7ce/Downloads/md_dev/Ventoy/INSTALL/ventoy";

    #[test]
    fn decompresses_ventoy_wimboot_helper() {
        let data = std::fs::read(std::format!("{}/wimboot.x86_64.xz", VENTOY_ASSET_DIR))
            .expect("Ventoy wimboot.x86_64.xz");

        let decoded = decompress_xz(&data, 2 * 1024 * 1024).expect("decode wimboot helper");

        assert_eq!(&decoded[..2], b"MZ");
        assert_eq!(decoded.len(), 48_480);
    }

    #[test]
    fn decompresses_ventoy_common_bcd() {
        let data = std::fs::read(std::format!("{}/common_bcd.xz", VENTOY_ASSET_DIR))
            .expect("Ventoy common_bcd.xz");

        let decoded = decompress_xz(&data, 2 * 1024 * 1024).expect("decode common BCD");

        assert_eq!(&decoded[..4], b"regf");
        assert_eq!(decoded.len(), 24 * 1024);
    }

    #[test]
    fn enforces_output_limit() {
        let data = std::fs::read(std::format!("{}/common_bcd.xz", VENTOY_ASSET_DIR))
            .expect("Ventoy common_bcd.xz");

        let err = decompress_xz(&data, 1024).expect_err("output limit");

        assert_eq!(err, XzDecodeError::OutputTooLarge);
    }
}
