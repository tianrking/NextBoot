use super::{
    huffman::LzxHuffman, lzx_footer_bits, LzxBlockType, LzxDecodeError, HUFFMAN_BITS,
    LZX_ALIGNOFFSET_BITS, LZX_ALIGNOFFSET_CODES, LZX_BLOCK_TYPE_BITS, LZX_DEFAULT_BLOCK_LEN,
    LZX_LENGTH_CODES, LZX_MAIN_CODES, LZX_MAIN_LIT_CODES, LZX_POSITION_BASE, LZX_POSITION_SLOTS,
    LZX_PRETREE_BITS, LZX_PRETREE_CODES, LZX_REPEATED_OFFSETS, LZX_WIM_MAGIC_FILESIZE,
};

pub(super) struct Lzx<'a, 'out> {
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
    pub(super) fn new(input: &'a [u8], output: &'out mut [u8]) -> Self {
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

    pub(super) fn read_block_header(&mut self) -> Result<(), LzxDecodeError> {
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

    pub(super) fn copy_uncompressed_block(&mut self) -> Result<(), LzxDecodeError> {
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

    pub(super) fn decode_token(&mut self) -> Result<(), LzxDecodeError> {
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

    pub(super) fn translate_jumps(&mut self) {
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

    pub(super) fn has_input(&self) -> bool {
        self.input_offset < self.input.len()
    }

    pub(super) fn is_uncompressed_block(&self) -> bool {
        self.block_type == LzxBlockType::Uncompressed
    }

    pub(super) fn block_finished(&self) -> bool {
        self.output_offset >= self.block_end
    }

    pub(super) fn output_offset(&self) -> usize {
        self.output_offset
    }
}
