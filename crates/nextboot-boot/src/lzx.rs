//! LZX decompression used by WIM resources.
//!
//! This mirrors the block format handled by Ventoy's bundled wimboot LZX
//! decoder, including WIM's E8 jump translation pass.

#[path = "lzx/decoder.rs"]
mod decoder;
#[path = "lzx/huffman.rs"]
mod huffman;

#[cfg(test)]
#[path = "lzx/tests.rs"]
mod tests;

pub(super) const HUFFMAN_BITS: usize = 16;
pub(super) const LZX_ALIGNOFFSET_CODES: usize = 8;
pub(super) const LZX_ALIGNOFFSET_BITS: u8 = 3;
pub(super) const LZX_PRETREE_CODES: usize = 20;
pub(super) const LZX_PRETREE_BITS: u8 = 4;
pub(super) const LZX_MAIN_LIT_CODES: usize = 256;
pub(super) const LZX_POSITION_SLOTS: usize = 30;
pub(super) const LZX_MAIN_CODES: usize = LZX_MAIN_LIT_CODES + (8 * LZX_POSITION_SLOTS);
pub(super) const LZX_LENGTH_CODES: usize = 249;
pub(super) const LZX_BLOCK_TYPE_BITS: u8 = 3;
pub(super) const LZX_DEFAULT_BLOCK_LEN: usize = 32 * 1024;
pub(super) const LZX_REPEATED_OFFSETS: usize = 3;
pub(super) const LZX_WIM_MAGIC_FILESIZE: i32 = 12_000_000;
pub(super) const LZX_POSITION_BASE: [usize; LZX_POSITION_SLOTS] = make_lzx_position_base();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LzxDecodeError {
    OddInputLength,
    InputTooShort,
    InvalidHuffmanLength,
    IncompleteHuffmanAlphabet,
    InvalidHuffmanCode,
    InvalidBlockType,
    InvalidPretreeRepeat,
    OutputOverflow,
    MatchUnderrun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LzxBlockType {
    Verbatim,
    AlignedOffset,
    Uncompressed,
}

pub fn decompress_lzx(input: &[u8], out: &mut [u8]) -> Result<usize, LzxDecodeError> {
    if input.len() % 2 != 0 {
        return Err(LzxDecodeError::OddInputLength);
    }

    let mut lzx = decoder::Lzx::new(input, out);
    while lzx.has_input() {
        lzx.read_block_header()?;
        if lzx.is_uncompressed_block() {
            lzx.copy_uncompressed_block()?;
        } else {
            while !lzx.block_finished() {
                lzx.decode_token()?;
            }
        }
    }

    lzx.translate_jumps();
    Ok(lzx.output_offset())
}

pub(super) const fn lzx_footer_bits(position_slot: usize) -> usize {
    if position_slot < 2 {
        0
    } else if position_slot < 38 {
        (position_slot / 2) - 1
    } else {
        17
    }
}

const fn make_lzx_position_base() -> [usize; LZX_POSITION_SLOTS] {
    let mut out = [0usize; LZX_POSITION_SLOTS];
    let mut index = 1usize;
    while index < LZX_POSITION_SLOTS {
        out[index] = out[index - 1] + (1usize << lzx_footer_bits(index - 1));
        index += 1;
    }
    out
}
