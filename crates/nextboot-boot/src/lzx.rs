//! LZX decompression used by WIM resources.
//!
//! This mirrors the block format handled by Ventoy's bundled wimboot LZX
//! decoder, including WIM's E8 jump translation pass.

const HUFFMAN_BITS: usize = 16;
const LZX_ALIGNOFFSET_CODES: usize = 8;
const LZX_ALIGNOFFSET_BITS: u8 = 3;
const LZX_PRETREE_CODES: usize = 20;
const LZX_PRETREE_BITS: u8 = 4;
const LZX_MAIN_LIT_CODES: usize = 256;
const LZX_POSITION_SLOTS: usize = 30;
const LZX_MAIN_CODES: usize = LZX_MAIN_LIT_CODES + (8 * LZX_POSITION_SLOTS);
const LZX_LENGTH_CODES: usize = 249;
const LZX_BLOCK_TYPE_BITS: u8 = 3;
const LZX_DEFAULT_BLOCK_LEN: usize = 32 * 1024;
const LZX_REPEATED_OFFSETS: usize = 3;
const LZX_WIM_MAGIC_FILESIZE: i32 = 12_000_000;
const LZX_POSITION_BASE: [usize; LZX_POSITION_SLOTS] = make_lzx_position_base();

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
enum LzxBlockType {
    Verbatim,
    AlignedOffset,
    Uncompressed,
}

pub fn decompress_lzx(input: &[u8], out: &mut [u8]) -> Result<usize, LzxDecodeError> {
    if input.len() % 2 != 0 {
        return Err(LzxDecodeError::OddInputLength);
    }

    let mut lzx = Lzx::new(input, out);
    while lzx.input_offset < lzx.input.len() {
        lzx.read_block_header()?;
        if lzx.block_type == LzxBlockType::Uncompressed {
            lzx.copy_uncompressed_block()?;
        } else {
            while lzx.output_offset < lzx.block_end {
                lzx.decode_token()?;
            }
        }
    }

    lzx.translate_jumps();
    Ok(lzx.output_offset)
}

struct Lzx<'a, 'out> {
    input: &'a [u8],
    input_offset: usize,
    output: &'out mut [u8],
    output_offset: usize,
    block_end: usize,
    accumulator: u32,
    bits: u8,
    block_type: LzxBlockType,
    repeated_offset: [usize; LZX_REPEATED_OFFSETS],
    alignoffset: LzxHuffman<LZX_ALIGNOFFSET_CODES>,
    pretree: LzxHuffman<LZX_PRETREE_CODES>,
    main: LzxHuffman<LZX_MAIN_CODES>,
    length: LzxHuffman<LZX_LENGTH_CODES>,
    main_lengths: [u8; LZX_MAIN_CODES],
    length_lengths: [u8; LZX_LENGTH_CODES],
}

impl<'a, 'out> Lzx<'a, 'out> {
    fn new(input: &'a [u8], output: &'out mut [u8]) -> Self {
        Self {
            input,
            input_offset: 0,
            output,
            output_offset: 0,
            block_end: 0,
            accumulator: 0,
            bits: 0,
            block_type: LzxBlockType::Verbatim,
            repeated_offset: [1; LZX_REPEATED_OFFSETS],
            alignoffset: LzxHuffman::empty(),
            pretree: LzxHuffman::empty(),
            main: LzxHuffman::empty(),
            length: LzxHuffman::empty(),
            main_lengths: [0; LZX_MAIN_CODES],
            length_lengths: [0; LZX_LENGTH_CODES],
        }
    }

    fn accumulate(&mut self, needed: u8) -> Result<u32, LzxDecodeError> {
        if needed as usize > HUFFMAN_BITS {
            return Err(LzxDecodeError::InvalidHuffmanCode);
        }
        if self.bits < needed && self.input_offset < self.input.len() {
            let word = self.read_u16()?;
            self.accumulator |= u32::from(word) << (16 - u32::from(self.bits));
            self.bits += 16;
        }
        Ok(self.accumulator >> 16)
    }

    fn consume(&mut self, bits: u8) -> Result<(), LzxDecodeError> {
        if bits > self.bits {
            return Err(LzxDecodeError::InputTooShort);
        }
        self.accumulator <<= u32::from(bits);
        self.bits -= bits;
        Ok(())
    }

    fn get_bits(&mut self, bits: u8) -> Result<usize, LzxDecodeError> {
        if bits == 0 {
            return Ok(0);
        }
        let normalized = self.accumulate(bits)?;
        self.consume(bits)?;
        Ok((normalized >> (HUFFMAN_BITS as u32 - u32::from(bits))) as usize)
    }

    fn align_for_bytes(&mut self, padding_bits: u8) -> Result<(), LzxDecodeError> {
        self.get_bits(padding_bits)?;
        self.consume(self.bits)
    }

    fn read_u8(&mut self) -> Result<u8, LzxDecodeError> {
        let byte = *self
            .input
            .get(self.input_offset)
            .ok_or(LzxDecodeError::InputTooShort)?;
        self.input_offset += 1;
        Ok(byte)
    }

    fn read_u16(&mut self) -> Result<u16, LzxDecodeError> {
        let bytes = self
            .input
            .get(self.input_offset..self.input_offset + 2)
            .ok_or(LzxDecodeError::InputTooShort)?;
        self.input_offset += 2;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, LzxDecodeError> {
        let bytes = self
            .input
            .get(self.input_offset..self.input_offset + 4)
            .ok_or(LzxDecodeError::InputTooShort)?;
        self.input_offset += 4;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_block_header(&mut self) -> Result<(), LzxDecodeError> {
        self.block_type = match self.get_bits(LZX_BLOCK_TYPE_BITS)? {
            1 => LzxBlockType::Verbatim,
            2 => LzxBlockType::AlignedOffset,
            3 => LzxBlockType::Uncompressed,
            _ => return Err(LzxDecodeError::InvalidBlockType),
        };

        let block_len = if self.get_bits(1)? != 0 {
            LZX_DEFAULT_BLOCK_LEN
        } else {
            let high = self.get_bits(8)?;
            let low = self.get_bits(8)?;
            (high << 8) | low
        };
        self.block_end = self
            .output_offset
            .checked_add(block_len)
            .ok_or(LzxDecodeError::OutputOverflow)?;
        if self.block_end > self.output.len() {
            return Err(LzxDecodeError::OutputOverflow);
        }

        match self.block_type {
            LzxBlockType::AlignedOffset => {
                self.alignoffset =
                    self.read_raw_alphabet::<LZX_ALIGNOFFSET_CODES>(LZX_ALIGNOFFSET_BITS)?;
                self.read_main_alphabet()?;
                self.read_length_alphabet()?;
            }
            LzxBlockType::Verbatim => {
                self.read_main_alphabet()?;
                self.read_length_alphabet()?;
            }
            LzxBlockType::Uncompressed => {
                self.align_for_bytes(1)?;
                for index in 0..LZX_REPEATED_OFFSETS {
                    self.repeated_offset[index] = self.read_u32()? as usize;
                }
            }
        }

        Ok(())
    }

    fn read_raw_alphabet<const N: usize>(
        &mut self,
        bits: u8,
    ) -> Result<LzxHuffman<N>, LzxDecodeError> {
        let mut lengths = [0u8; N];
        for len in &mut lengths {
            *len = self.get_bits(bits)? as u8;
        }
        LzxHuffman::from_lengths(&lengths)
    }

    fn read_pretree_lengths<const N: usize>(
        &mut self,
        previous: &[u8; N],
    ) -> Result<[u8; N], LzxDecodeError> {
        self.pretree = self.read_raw_alphabet::<LZX_PRETREE_CODES>(LZX_PRETREE_BITS)?;

        let mut lengths = *previous;
        let mut duplicate = 0usize;
        for index in 0..N {
            if duplicate != 0 {
                if index == 0 {
                    return Err(LzxDecodeError::InvalidPretreeRepeat);
                }
                lengths[index] = lengths[index - 1];
                duplicate -= 1;
                continue;
            }

            let code = self.decode_pretree()?;
            let length = if code <= 16 {
                (usize::from(lengths[index]) + 17 - code) % 17
            } else if code == 17 {
                duplicate = self.get_bits(4)? + 3;
                0
            } else if code == 18 {
                duplicate = self.get_bits(5)? + 19;
                0
            } else if code == 19 {
                duplicate = self.get_bits(1)? + 3;
                let code = self.decode_pretree()?;
                if code > 16 {
                    return Err(LzxDecodeError::InvalidPretreeRepeat);
                }
                (usize::from(lengths[index]) + 17 - code) % 17
            } else {
                return Err(LzxDecodeError::InvalidPretreeRepeat);
            };
            lengths[index] = length as u8;
        }

        if duplicate != 0 {
            return Err(LzxDecodeError::InvalidPretreeRepeat);
        }
        Ok(lengths)
    }

    fn read_main_alphabet(&mut self) -> Result<(), LzxDecodeError> {
        let mut literal_previous = [0u8; LZX_MAIN_LIT_CODES];
        literal_previous.copy_from_slice(&self.main_lengths[..LZX_MAIN_LIT_CODES]);
        let literal_lengths = self.read_pretree_lengths(&literal_previous)?;
        self.main_lengths[..LZX_MAIN_LIT_CODES].copy_from_slice(&literal_lengths);

        let mut remainder_previous = [0u8; LZX_MAIN_CODES - LZX_MAIN_LIT_CODES];
        remainder_previous.copy_from_slice(&self.main_lengths[LZX_MAIN_LIT_CODES..]);
        let remainder_lengths = self.read_pretree_lengths(&remainder_previous)?;
        self.main_lengths[LZX_MAIN_LIT_CODES..].copy_from_slice(&remainder_lengths);

        self.main = LzxHuffman::from_lengths(&self.main_lengths)?;
        Ok(())
    }

    fn read_length_alphabet(&mut self) -> Result<(), LzxDecodeError> {
        let previous = self.length_lengths;
        self.length_lengths = self.read_pretree_lengths(&previous)?;
        self.length = LzxHuffman::from_lengths(&self.length_lengths)?;
        Ok(())
    }

    fn decode_pretree(&mut self) -> Result<usize, LzxDecodeError> {
        let huf = self.accumulate(HUFFMAN_BITS as u8)?;
        let (raw, len) = self.pretree.decode(huf)?;
        self.consume(len)?;
        Ok(usize::from(raw))
    }

    fn decode_alignoffset(&mut self) -> Result<usize, LzxDecodeError> {
        let huf = self.accumulate(HUFFMAN_BITS as u8)?;
        let (raw, len) = self.alignoffset.decode(huf)?;
        self.consume(len)?;
        Ok(usize::from(raw))
    }

    fn decode_main(&mut self) -> Result<usize, LzxDecodeError> {
        let huf = self.accumulate(HUFFMAN_BITS as u8)?;
        let (raw, len) = self.main.decode(huf)?;
        self.consume(len)?;
        Ok(usize::from(raw))
    }

    fn decode_length(&mut self) -> Result<usize, LzxDecodeError> {
        let huf = self.accumulate(HUFFMAN_BITS as u8)?;
        let (raw, len) = self.length.decode(huf)?;
        self.consume(len)?;
        Ok(usize::from(raw))
    }

    fn copy_uncompressed_block(&mut self) -> Result<(), LzxDecodeError> {
        let len = self
            .block_end
            .checked_sub(self.output_offset)
            .ok_or(LzxDecodeError::OutputOverflow)?;
        let source_end = self
            .input_offset
            .checked_add(len)
            .ok_or(LzxDecodeError::InputTooShort)?;
        let source = self
            .input
            .get(self.input_offset..source_end)
            .ok_or(LzxDecodeError::InputTooShort)?;
        self.output[self.output_offset..self.block_end].copy_from_slice(source);
        self.input_offset = source_end;
        self.output_offset = self.block_end;
        if len % 2 != 0 {
            self.input_offset = self
                .input_offset
                .checked_add(1)
                .ok_or(LzxDecodeError::InputTooShort)?;
            if self.input_offset > self.input.len() {
                return Err(LzxDecodeError::InputTooShort);
            }
        }
        Ok(())
    }

    fn decode_token(&mut self) -> Result<(), LzxDecodeError> {
        let mut main = self.decode_main()?;
        if main < LZX_MAIN_LIT_CODES {
            if self.output_offset >= self.output.len() {
                return Err(LzxDecodeError::OutputOverflow);
            }
            self.output[self.output_offset] = main as u8;
            self.output_offset += 1;
            return Ok(());
        }

        main -= LZX_MAIN_LIT_CODES;
        let length_header = main & 7;
        let length = if length_header == 7 {
            self.decode_length()?
        } else {
            0
        };
        let match_length = length_header
            .checked_add(2)
            .and_then(|value| value.checked_add(length))
            .ok_or(LzxDecodeError::OutputOverflow)?;

        let position_slot = main >> 3;
        if position_slot >= LZX_POSITION_SLOTS {
            return Err(LzxDecodeError::InvalidHuffmanCode);
        }

        let match_offset = if position_slot < LZX_REPEATED_OFFSETS {
            let offset = self.repeated_offset[position_slot];
            self.repeated_offset[position_slot] = self.repeated_offset[0];
            self.repeated_offset[0] = offset;
            offset
        } else {
            let offset_bits = lzx_footer_bits(position_slot);
            let (verbatim_bits, aligned_bits) =
                if self.block_type == LzxBlockType::AlignedOffset && offset_bits >= 3 {
                    (
                        self.get_bits((offset_bits - 3) as u8)? << 3,
                        self.decode_alignoffset()?,
                    )
                } else {
                    (self.get_bits(offset_bits as u8)?, 0)
                };
            let offset = LZX_POSITION_BASE[position_slot]
                .checked_add(verbatim_bits)
                .and_then(|value| value.checked_add(aligned_bits))
                .and_then(|value| value.checked_sub(2))
                .ok_or(LzxDecodeError::MatchUnderrun)?;

            for index in (1..LZX_REPEATED_OFFSETS).rev() {
                self.repeated_offset[index] = self.repeated_offset[index - 1];
            }
            self.repeated_offset[0] = offset;
            offset
        };

        if match_offset == 0 || match_offset > self.output_offset {
            return Err(LzxDecodeError::MatchUnderrun);
        }
        let end = self
            .output_offset
            .checked_add(match_length)
            .ok_or(LzxDecodeError::OutputOverflow)?;
        if end > self.output.len() || end > self.block_end {
            return Err(LzxDecodeError::OutputOverflow);
        }
        while self.output_offset < end {
            let byte = self.output[self.output_offset - match_offset];
            self.output[self.output_offset] = byte;
            self.output_offset += 1;
        }

        Ok(())
    }

    fn translate_jumps(&mut self) {
        if self.output_offset < 10 {
            return;
        }

        let mut offset = 0usize;
        while offset < self.output_offset - 10 {
            if self.output[offset] == 0xe8 {
                let mut bytes = [0u8; 4];
                bytes.copy_from_slice(&self.output[offset + 1..offset + 5]);
                let mut target = i32::from_le_bytes(bytes);
                if target >= 0 {
                    if target < LZX_WIM_MAGIC_FILESIZE {
                        target -= offset as i32;
                    }
                } else if target >= -(offset as i32) {
                    target += LZX_WIM_MAGIC_FILESIZE;
                }
                self.output[offset + 1..offset + 5].copy_from_slice(&target.to_le_bytes());
                offset += 4;
            }
            offset += 1;
        }
    }
}

struct LzxHuffman<const N: usize> {
    counts: [u16; HUFFMAN_BITS + 1],
    starts: [u32; HUFFMAN_BITS + 1],
    first_symbol: [usize; HUFFMAN_BITS + 1],
    symbols: [u16; N],
}

impl<const N: usize> LzxHuffman<N> {
    fn empty() -> Self {
        Self {
            counts: [0; HUFFMAN_BITS + 1],
            starts: [0; HUFFMAN_BITS + 1],
            first_symbol: [0; HUFFMAN_BITS + 1],
            symbols: [0; N],
        }
    }

    fn from_lengths(lengths: &[u8; N]) -> Result<Self, LzxDecodeError> {
        let mut out = Self::empty();
        let mut empty = true;
        for len in lengths {
            let len = usize::from(*len);
            if len > HUFFMAN_BITS {
                return Err(LzxDecodeError::InvalidHuffmanLength);
            }
            if len != 0 {
                out.counts[len] += 1;
                empty = false;
            }
        }
        if empty {
            out.counts[1] = 2;
        }

        let mut huf = 0u32;
        let mut cumulative = 0usize;
        for bits in 1..=HUFFMAN_BITS {
            out.starts[bits] = huf << (HUFFMAN_BITS - bits);
            out.first_symbol[bits] = cumulative;
            huf = huf
                .checked_add(u32::from(out.counts[bits]))
                .ok_or(LzxDecodeError::IncompleteHuffmanAlphabet)?;
            if huf > (1u32 << bits) {
                return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
            }
            huf <<= 1;
            cumulative = cumulative
                .checked_add(usize::from(out.counts[bits]))
                .ok_or(LzxDecodeError::IncompleteHuffmanAlphabet)?;
            if cumulative > N {
                return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
            }
        }
        if huf != (1u32 << (HUFFMAN_BITS + 1)) {
            return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
        }

        if !empty {
            let mut cursor = out.first_symbol;
            for (symbol, len) in lengths.iter().copied().enumerate() {
                if len == 0 {
                    continue;
                }
                let len = usize::from(len);
                out.symbols[cursor[len]] = symbol as u16;
                cursor[len] += 1;
            }
        }

        Ok(out)
    }

    fn decode(&self, huf: u32) -> Result<(u16, u8), LzxDecodeError> {
        for bits in 1..=HUFFMAN_BITS {
            let count = u32::from(self.counts[bits]);
            if count == 0 {
                continue;
            }
            let shift = HUFFMAN_BITS - bits;
            let start = self.starts[bits];
            let end = start + (count << shift);
            if huf >= start && huf < end {
                let index = self.first_symbol[bits] + ((huf - start) >> shift) as usize;
                return Ok((self.symbols[index], bits as u8));
            }
        }

        Err(LzxDecodeError::InvalidHuffmanCode)
    }
}

const fn lzx_footer_bits(position_slot: usize) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn decompresses_uncompressed_block() {
        let compressed = make_lzx_uncompressed_block(b"hello");
        let mut out = [0u8; 5];

        assert_eq!(decompress_lzx(&compressed, &mut out), Ok(5));
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn rejects_odd_length_input() {
        let mut out = [0u8; 1];

        assert_eq!(
            decompress_lzx(&[0, 1, 2], &mut out),
            Err(LzxDecodeError::OddInputLength)
        );
    }

    #[test]
    fn decompresses_verbatim_literal_block() {
        let compressed = make_lzx_verbatim_literal_block();
        let mut out = [0u8; 2];

        assert_eq!(decompress_lzx(&compressed, &mut out), Ok(2));
        assert_eq!(&out, b"hi");
    }

    fn make_lzx_uncompressed_block(bytes: &[u8]) -> Vec<u8> {
        let mut bits = Vec::new();
        push_bits(&mut bits, 3, 3);
        push_bits(&mut bits, 0, 1);
        push_bits(&mut bits, ((bytes.len() >> 8) & 0xff) as u16, 8);
        push_bits(&mut bits, (bytes.len() & 0xff) as u16, 8);
        push_bits(&mut bits, 0, 1);
        while bits.len() % 16 != 0 {
            bits.push(0);
        }

        let mut out = bits_to_le_words(&bits);
        for _ in 0..LZX_REPEATED_OFFSETS {
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out.extend_from_slice(bytes);
        if bytes.len() % 2 != 0 {
            out.push(0);
        }
        out
    }

    fn make_lzx_verbatim_literal_block() -> Vec<u8> {
        let mut bits = Vec::new();
        push_bits(&mut bits, 1, 3);
        push_bits(&mut bits, 0, 1);
        push_bits(&mut bits, 0, 8);
        push_bits(&mut bits, 2, 8);

        push_pretree_lengths(&mut bits, &[0, 16]);
        for symbol in 0..LZX_MAIN_LIT_CODES {
            bits.push(u8::from(
                symbol == usize::from(b'h') || symbol == usize::from(b'i'),
            ));
        }

        push_pretree_lengths(&mut bits, &[0, 1]);
        for _ in 0..(LZX_MAIN_CODES - LZX_MAIN_LIT_CODES) {
            bits.push(0);
        }

        push_pretree_lengths(&mut bits, &[0, 1]);
        for _ in 0..LZX_LENGTH_CODES {
            bits.push(0);
        }

        bits.push(0);
        bits.push(1);
        while bits.len() % 16 != 0 {
            bits.push(0);
        }

        bits_to_le_words(&bits)
    }

    fn push_pretree_lengths(bits: &mut Vec<u8>, active_codes: &[usize]) {
        for code in 0..LZX_PRETREE_CODES {
            push_bits(bits, u16::from(active_codes.contains(&code)), 4);
        }
    }

    fn push_bits(bits: &mut Vec<u8>, value: u16, count: usize) {
        for index in (0..count).rev() {
            bits.push(((value >> index) & 1) as u8);
        }
    }

    fn bits_to_le_words(bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in bits.chunks_exact(16) {
            let mut word = 0u16;
            for bit in chunk {
                word = (word << 1) | u16::from(*bit);
            }
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }
}
